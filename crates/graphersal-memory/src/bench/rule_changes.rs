//! `bench rule-changes`: does the next session follow a rule that changed? (M12, the design of
//! experiment A3.) Built-in scenarios, never the user's memory: each is an old rule written in
//! one session and a newer one replacing it in the next, then a question that needs the rule.
//!
//! - **Arms**, each a store of its own in the run directory (one project per scenario, a few
//!   unrelated memories beside the two rules):
//!   - `plain`: both memories, nothing links them (graphersal-memory before M12);
//!   - `marked`: the newer one written with `supersedes: [old]`, so every search marks the old
//!     one "SUPERSEDED by <new> on <date>:" (the product since M12);
//!   - `file`: `marked`, but the old rule still sits in a memory file the harness loads into the
//!     context (the risk M12 names: a file the user did not edit).
//! - **Calls**: the question through `claude -p` in a sandbox like the graph arm of `bench`
//!   (`mcp --no-daemon` over the arm's store, the read-only memory functions only, no settings,
//!   hooks or files of the user's); a [`super::DryRun`] for `--dry-run`.
//! - **Verdict** without a judge call: the answer names the newer rule only (`new`), the old one
//!   only (`old`), both, or neither, by the scenario's markers ([`verdict`]).
//! - **Cost guard** as `bench`: the estimate first, refused above `--budget`, each call capped
//!   with `--max-budget-usd`, the run stops when the next call could pass the budget.
//! - **Report**: `report.md` and `report.json` in `<data dir>/bench/<timestamp>-rule-changes/`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

use super::{ANSWER_CAP_USD, ANSWER_USD, Call, Harness, Kind, Usage};
use crate::commands::EXIT_FAILED;

/// One memory of a scenario: slug, kind, summary.
pub type Rule = (&'static str, &'static str, &'static str);

/// One rule change.
#[derive(Debug, Clone, Copy)]
pub struct Scenario {
    /// A short id.
    pub id: &'static str,
    /// The rule of the first session.
    pub old: Rule,
    /// The rule of the second session, replacing it.
    pub new: Rule,
    /// The question of the third session.
    pub question: &'static str,
    /// Words that only an answer following the newer rule contains (lower case).
    pub new_markers: &'static [&'static str],
    /// Words that only an answer following the old rule contains (lower case).
    pub old_markers: &'static [&'static str],
}

/// The built-in scenarios (A3's, without the one about the author's own setup).
pub const SCENARIOS: [Scenario; 5] = [
    Scenario {
        id: "package-manager",
        old: (
            "use-npm-install",
            "project",
            "Install dependencies with npm install; the lockfile is package-lock.json.",
        ),
        new: (
            "pnpm-instead-of-npm",
            "project",
            "Since 2026-11-03 we use pnpm instead of npm; run pnpm install (pnpm-lock.yaml).",
        ),
        question: "What exact command do I run to install this project's dependencies? Answer \
                   with the command and one sentence.",
        new_markers: &["pnpm"],
        old_markers: &["npm install", "npm ci", "npm i ", "package-lock"],
    },
    Scenario {
        id: "state-library",
        old: (
            "state-redux",
            "project",
            "Frontend state management uses Redux Toolkit slices.",
        ),
        new: (
            "zustand-replaces-redux",
            "project",
            "We moved from Redux to Zustand on 2026-11-05; new code uses Zustand stores.",
        ),
        question: "I need a new global store for the wishlist feature. Which state library \
                   should I use? One sentence.",
        new_markers: &["zustand"],
        old_markers: &["redux toolkit", "createslice", "redux slice", "use redux"],
    },
    Scenario {
        id: "node-version",
        old: (
            "node-18",
            "project",
            "The project targets Node 18; Docker images use node:18-alpine.",
        ),
        new: (
            "node-22-upgrade",
            "project",
            "Upgraded to Node 22 LTS on 2026-11-04; Node 18 is no longer supported.",
        ),
        question: "Which Node base image should the new worker's Dockerfile use? Answer with the \
                   image tag.",
        new_markers: &["22"],
        old_markers: &["node:18", "node 18"],
    },
    Scenario {
        id: "staging-host",
        old: (
            "staging-heroku",
            "reference",
            "Staging is hosted on Heroku (app shop-staging); deploy with git push heroku main.",
        ),
        new: (
            "staging-fly",
            "reference",
            "Since 2026-11-02 staging runs on Fly.io (app shop-staging); deploy with fly deploy.",
        ),
        question: "How do I deploy my branch to staging? Give the command.",
        new_markers: &["fly"],
        old_markers: &["heroku"],
    },
    Scenario {
        id: "coverage-gate",
        old: (
            "coverage-70",
            "project",
            "The minimum test coverage for a PR is 70 %.",
        ),
        new: (
            "coverage-85",
            "project",
            "The coverage gate was raised to 85 % on 2026-11-06.",
        ),
        question: "What test coverage does my PR need to pass the gate? Just the number.",
        new_markers: &["85"],
        old_markers: &["70"],
    },
];

/// Unrelated memories beside the two rules of every scenario, so the search has to choose.
const NOISE: [Rule; 8] = [
    (
        "repo-layout",
        "reference",
        "The monorepo has apps/web, apps/api and packages/ui.",
    ),
    (
        "typescript-strict",
        "project",
        "TypeScript runs in strict mode; no implicit any.",
    ),
    (
        "email-templates-mjml",
        "project",
        "Transactional emails are written in MJML under apps/api/emails.",
    ),
    (
        "load-tests-k6",
        "reference",
        "Load tests use k6; scripts live in tools/load.",
    ),
    (
        "dependabot-weekly",
        "project",
        "Dependabot opens update PRs weekly on Mondays.",
    ),
    (
        "feature-flags",
        "project",
        "Feature flags are read from the flags table, cached for one minute.",
    ),
    (
        "code-review-two",
        "feedback",
        "Every PR needs two approving reviews before merge.",
    ),
    (
        "logs-in-grafana",
        "reference",
        "Production logs are searched in Grafana Loki.",
    ),
];

/// One arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// Both memories, unlinked.
    Plain,
    /// The newer one supersedes the old one.
    Marked,
    /// Marked, with the old rule in a loaded memory file.
    File,
}

impl Arm {
    /// Every arm, in report order.
    pub const ALL: [Arm; 3] = [Arm::Plain, Arm::Marked, Arm::File];

    /// The arm's name.
    pub fn name(self) -> &'static str {
        match self {
            Arm::Plain => "plain",
            Arm::Marked => "marked",
            Arm::File => "file",
        }
    }
}

/// What a run works with.
#[derive(Debug, Clone)]
pub struct Options {
    /// The data directory (the run directory goes under `bench/`; the user's store is never
    /// opened).
    pub data_dir: PathBuf,
    /// This binary (the arms' MCP server).
    pub exe: PathBuf,
    /// The harness command (`claude -p ...`).
    pub harness_command: Vec<String>,
    /// How often every question is asked per arm.
    pub repeats: u32,
    /// The budget, USD.
    pub budget_usd: f64,
    /// A dry run (the caller passes a [`super::DryRun`]).
    pub dry_run: bool,
    /// Do not ask.
    pub yes: bool,
}

/// Whether `text` holds `marker` at a word start (so `pnpm install` does not count as `npm
/// install`).
fn holds(text: &str, marker: &str) -> bool {
    text.match_indices(marker).any(|(at, _)| {
        text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

/// The verdict of an answer: `new`, `old`, `both` or `neither`.
pub fn verdict(answer: &str, scenario: &Scenario) -> &'static str {
    let text = answer.to_lowercase();
    let new = scenario.new_markers.iter().any(|m| holds(&text, m));
    let old = scenario.old_markers.iter().any(|m| holds(&text, m));
    match (new, old) {
        (true, false) => "new",
        (false, true) => "old",
        (true, true) => "both",
        (false, false) => "neither",
    }
}

/// The system prompt of every call (the file arm adds the loaded file).
const SYSTEM: &str = "You are a coding agent working in the user's repository; the files are not \
    available in this session. Your long-term memory of this user and project is the \
    graphersal-memory MCP server. Before you answer a question about this project's conventions \
    or decisions, search the memory. Answer briefly.";

/// The project directory of `scenario` in an arm's directory.
fn project_dir(arm_dir: &Path, scenario: &Scenario) -> PathBuf {
    arm_dir.join("projects").join(scenario.id)
}

/// Writes the arm's store in `<arm_dir>/data`: per scenario, a project with the noise, the old
/// rule, then the newer one (with `supersedes` in the marked and file arms).
pub fn seed(arm_dir: &Path, arm: Arm) -> Result<(), String> {
    let data = arm_dir.join("data");
    let runtime = crate::runtime::mcp::StoreRuntime::open_with(
        crate::runtime::registry::DEFAULT_STORE,
        &crate::runtime::store::default_store_dir(&data),
        Arc::new(crate::runtime::mcp::endpoint()),
        &crate::runtime::maintenance::Settings::default(),
    )?;
    let result = (|| {
        for scenario in &SCENARIOS {
            let dir = project_dir(arm_dir, scenario);
            std::fs::create_dir_all(&dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            let key = crate::runtime::mcp::project_key(Some(&dir), &dir);
            let project = crate::memory::projects::resolve(&data, &key)?.id;
            let remember = |rule: &Rule, supersedes: Vec<&str>| {
                runtime.call(
                    "remember",
                    json!({"slug": rule.0, "kind": rule.1, "summary": rule.2, "body": rule.2,
                           "supersedes": supersedes, "scope": "project", "project": project}),
                )
            };
            for rule in &NOISE {
                remember(rule, Vec::new())?;
            }
            remember(&scenario.old, Vec::new())?;
            let supersedes = match arm {
                Arm::Plain => Vec::new(),
                Arm::Marked | Arm::File => vec![scenario.old.0],
            };
            remember(&scenario.new, supersedes)?;
        }
        Ok::<(), String>(())
    })();
    let closed = runtime.close();
    result.and(closed)
}

/// The arguments of a call of `scenario` in `arm` (after the harness command), with the
/// sandbox files written into `arm_dir`.
fn call_args(
    options: &Options,
    arm_dir: &Path,
    arm: Arm,
    scenario: &Scenario,
) -> Result<Vec<String>, String> {
    let files = arm_dir.join(format!("calls/{}", scenario.id));
    std::fs::create_dir_all(&files)
        .map_err(|err| format!("cannot create {}: {err}", files.display()))?;
    let settings = files.join("settings.json");
    let mcp = files.join("mcp.json");
    let system = files.join("system.md");
    let server = super::arms::SERVER;
    let pretty = |value: &Value| {
        format!(
            "{}\n",
            crate::install::jsonedit::render(value, Some("  "), 0)
        )
    };
    crate::fsutil::write_atomic(
        &settings,
        pretty(&crate::harnesses::claude_code::bench::settings()).as_bytes(),
    )?;
    let data = arm_dir.join("data");
    let project = project_dir(arm_dir, scenario);
    crate::fsutil::write_atomic(
        &mcp,
        pretty(&json!({"mcpServers": {server: {
            "type": "stdio",
            "command": options.exe.display().to_string(),
            "args": ["mcp", "--no-daemon", "--data-dir", data.display().to_string(),
                     "--project", project.display().to_string()],
            "env": {crate::memory::summaries::HOOK_ENV: "off"},
        }}}))
        .as_bytes(),
    )?;
    let mut text = SYSTEM.to_string();
    if arm == Arm::File {
        text.push_str(&format!(
            "\n\nContents of the project memory file (loaded by the harness at session start):\n\
             - [{}]: {}\n",
            scenario.old.0, scenario.old.2
        ));
    }
    crate::fsutil::write_atomic(&system, text.as_bytes())?;
    let mut args = crate::harnesses::claude_code::bench::common_args(&settings, &mcp);
    args.extend([
        "--tools".to_string(),
        String::new(),
        "--allowedTools".to_string(),
        ["memories", "memory", "recall"]
            .iter()
            .map(|f| format!("mcp__{server}__{f}"))
            .collect::<Vec<_>>()
            .join(","),
        "--append-system-prompt-file".to_string(),
        system.display().to_string(),
    ]);
    Ok(args)
}

/// A new run directory `<data dir>/bench/<timestamp>-rule-changes`.
fn new_run_dir(data_dir: &Path, started_at: &str) -> Result<PathBuf, String> {
    let stamp: String = started_at
        .chars()
        .filter(|c| *c != ':' && *c != '-')
        .collect();
    let base = data_dir.join("bench");
    let mut dir = base.join(format!("{stamp}-rule-changes"));
    let mut n = 2;
    while dir.exists() {
        dir = base.join(format!("{stamp}-rule-changes-{n}"));
        n += 1;
    }
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// One answer of the run.
#[derive(Debug, Clone)]
struct Row {
    arm: Arm,
    scenario: &'static str,
    repeat: u32,
    verdict: &'static str,
    usage: Usage,
}

/// Runs the rule-change bench.
pub fn run(
    options: &Options,
    harness: &mut dyn Harness,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let is_claude = options.harness_command.first().is_some_and(|program| {
        Path::new(program)
            .file_stem()
            .is_some_and(|stem| stem == "claude")
    });
    if !is_claude {
        return Err(format!(
            "bench rule-changes runs through Claude Code's `claude -p` only for now; the harness \
             command is `{}` (summaries.harness_command in config.json)",
            options.harness_command.join(" ")
        ));
    }
    let calls = SCENARIOS.len() * Arm::ALL.len() * options.repeats as usize;
    let estimate = calls as f64 * ANSWER_USD;
    let _ = writeln!(
        out,
        "bench rule-changes: {} built-in rule changes x {} arms (plain, marked, file) x {} \
         repeat(s) = {calls} answers; no judge call (the answer's words decide).",
        SCENARIOS.len(),
        Arm::ALL.len(),
        options.repeats
    );
    let _ = writeln!(
        out,
        "Estimate (list price): about ${estimate:.2}. Budget ${:.2}.",
        options.budget_usd
    );
    if estimate > options.budget_usd {
        let _ = writeln!(
            out,
            "Refused: the estimate is above the budget. Raise it with --budget, or use fewer \
             --repeats."
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
    for arm in Arm::ALL {
        seed(&run_dir.join("arms").join(arm.name()), arm)?;
    }
    let _ = writeln!(out, "Run in {}", run_dir.display());
    let mut rows: Vec<Row> = Vec::new();
    let mut spent = 0.0;
    let mut stopped = None;
    'run: for repeat in 1..=options.repeats {
        for scenario in &SCENARIOS {
            for arm in Arm::ALL {
                let left = options.budget_usd - spent;
                if left < ANSWER_USD {
                    stopped = Some(format!(
                        "the budget of ${:.2} was reached after {} answer(s)",
                        options.budget_usd,
                        rows.len()
                    ));
                    break 'run;
                }
                let arm_dir = run_dir.join("arms").join(arm.name());
                let mut args = options.harness_command.clone();
                args.extend(call_args(options, &arm_dir, arm, scenario)?);
                args.extend([
                    "--max-budget-usd".to_string(),
                    format!("{:.2}", ANSWER_CAP_USD.min(left)),
                ]);
                let cwd = arm_dir.join("cwd");
                std::fs::create_dir_all(&cwd)
                    .map_err(|err| format!("cannot create {}: {err}", cwd.display()))?;
                let started = Instant::now();
                let value = harness.call(&Call {
                    kind: Kind::Answer,
                    args,
                    input: format!(
                        "[benchmark rule-changes r{repeat} {started_at}]\n{}",
                        scenario.question
                    ),
                    cwd,
                    env: Vec::new(),
                });
                let usage = Usage::of(&value, started.elapsed());
                spent += usage.cost_usd;
                rows.push(Row {
                    arm,
                    scenario: scenario.id,
                    repeat,
                    verdict: verdict(&usage.text, scenario),
                    usage,
                });
            }
        }
    }
    let report = report_json(&rows, spent, stopped.as_deref(), options);
    crate::fsutil::write_atomic(
        &run_dir.join("report.json"),
        format!(
            "{}\n",
            crate::install::jsonedit::render(&report, Some("  "), 0)
        )
        .as_bytes(),
    )?;
    let markdown = report_markdown(&report);
    crate::fsutil::write_atomic(&run_dir.join("report.md"), markdown.as_bytes())?;
    for arm in Arm::ALL {
        // The stores are scratch: the report keeps what was asked and answered.
        let _ = std::fs::remove_dir_all(run_dir.join("arms").join(arm.name()).join("data"));
    }
    let _ = write!(out, "{markdown}");
    let _ = writeln!(out, "Report: {}", run_dir.join("report.md").display());
    Ok(0)
}

/// The report: per arm how many answers follow the newer rule, and every answer.
fn report_json(rows: &[Row], spent: f64, stopped: Option<&str>, options: &Options) -> Value {
    let arms: Vec<Value> = Arm::ALL
        .iter()
        .map(|arm| {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.arm == *arm).collect();
            let count = |v: &str| mine.iter().filter(|r| r.verdict == v).count();
            json!({"arm": arm.name(), "answers": mine.len(), "new": count("new"),
                   "old": count("old"), "both": count("both"), "neither": count("neither"),
                   "errors": mine.iter().filter(|r| r.usage.error.is_some()).count()})
        })
        .collect();
    let answers: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({"arm": r.arm.name(), "scenario": r.scenario, "repeat": r.repeat,
                   "verdict": r.verdict, "answer": r.usage.text, "error": r.usage.error,
                   "cost_usd": r.usage.cost_usd, "turns": r.usage.turns})
        })
        .collect();
    json!({"kind": "rule-changes", "harness_command": options.harness_command,
           "dry_run": options.dry_run, "repeats": options.repeats, "spent_usd": spent,
           "stopped": stopped, "arms": arms, "answers": answers})
}

/// The report as Markdown.
fn report_markdown(report: &Value) -> String {
    let mut text = String::from(
        "# bench rule-changes\n\nA rule changed between two sessions; does the next answer \
         follow the newer one?\n\n| arm | follows the new rule | old | both | neither | errors \
         |\n|---|---|---|---|---|---|\n",
    );
    for arm in report["arms"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "| {} | {}/{} | {} | {} | {} | {} |\n",
            arm["arm"].as_str().unwrap_or_default(),
            arm["new"],
            arm["answers"],
            arm["old"],
            arm["both"],
            arm["neither"],
            arm["errors"]
        ));
    }
    text.push_str(&format!(
        "\nplain: both memories, unlinked; marked: the newer one supersedes the old one (marked \
         in every search); file: marked, with the old rule in a loaded memory file.\nSpent: \
         ${:.3}{}.\n",
        report["spent_usd"].as_f64().unwrap_or(0.0),
        if report["dry_run"] == true {
            " (dry run: a fake harness answered)"
        } else {
            ""
        }
    ));
    if let Some(why) = report["stopped"].as_str() {
        text.push_str(&format!("Stopped early: {why}.\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::mcp_tests::TempDir;

    #[test]
    fn the_answer_words_give_the_verdict() {
        let pm = &SCENARIOS[0];
        assert_eq!(verdict("Run `pnpm install`.", pm), "new");
        assert_eq!(verdict("Run npm install.", pm), "old");
        assert_eq!(verdict("Use pnpm, not npm install.", pm), "both");
        assert_eq!(verdict("I do not know.", pm), "neither");
        assert_eq!(verdict("node:22-alpine", &SCENARIOS[2]), "new");
        assert_eq!(verdict("85 %", &SCENARIOS[4]), "new");
    }

    #[test]
    fn a_dry_run_asks_every_scenario_in_every_arm_and_spends_nothing() {
        let dir = TempDir::new("rulerun");
        let options = Options {
            data_dir: dir.path().to_path_buf(),
            exe: PathBuf::from("/opt/gm/graphersal-memory"),
            harness_command: vec!["claude".into(), "-p".into()],
            repeats: 1,
            budget_usd: 3.0,
            dry_run: true,
            yes: true,
        };
        let mut harness = super::super::DryRun::default();
        let mut out = Vec::new();
        let code = run(&options, &mut harness, &mut &b""[..], &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(code, 0, "{out}");
        assert_eq!(harness.calls.len(), SCENARIOS.len() * Arm::ALL.len());
        for call in &harness.calls {
            assert!(call.args.contains(&"--strict-mcp-config".to_string()));
            assert!(call.args.contains(&"--max-budget-usd".to_string()));
        }
        assert!(out.contains("| plain | 0/5 |"), "{out}");
        assert!(out.contains("Spent: $0.000 (dry run"), "{out}");
        // Nothing but the run directory was written into the data directory.
        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, ["bench"]);

        let refused = Options {
            budget_usd: 0.1,
            ..options.clone()
        };
        let mut out = Vec::new();
        let code = run(&refused, &mut harness, &mut &b""[..], &mut out).unwrap();
        assert_eq!(code, EXIT_FAILED);
        let other = Options {
            harness_command: vec!["opencode".into(), "run".into()],
            ..options
        };
        assert!(run(&other, &mut harness, &mut &b""[..], &mut Vec::new()).is_err());
    }

    #[test]
    fn the_arms_differ_only_in_the_link_and_the_file() {
        let dir = TempDir::new("rulechg");
        for arm in Arm::ALL {
            seed(&dir.path().join(arm.name()), arm).unwrap();
        }
        for arm in Arm::ALL {
            let arm_dir = dir.path().join(arm.name());
            let data = arm_dir.join("data");
            let runtime = crate::runtime::mcp::StoreRuntime::open(
                crate::runtime::registry::DEFAULT_STORE,
                &crate::runtime::store::default_store_dir(&data),
                Arc::new(crate::runtime::mcp::endpoint()),
            )
            .unwrap();
            let scenario = &SCENARIOS[0];
            let key = crate::runtime::mcp::project_key(
                Some(&project_dir(&arm_dir, scenario)),
                &project_dir(&arm_dir, scenario),
            );
            let project = crate::memory::projects::resolve(&data, &key).unwrap().id;
            let old = runtime
                .call(
                    "memory",
                    json!({"slug": scenario.old.0, "project": project}),
                )
                .unwrap();
            runtime.close().unwrap();
            let marked = old["summary"]
                .as_str()
                .unwrap()
                .starts_with("SUPERSEDED by ");
            assert_eq!(marked, arm != Arm::Plain, "{}: {old}", arm.name());
            let options = Options {
                data_dir: dir.path().to_path_buf(),
                exe: PathBuf::from("/opt/gm/graphersal-memory"),
                harness_command: vec!["claude".into(), "-p".into()],
                repeats: 1,
                budget_usd: 1.0,
                dry_run: true,
                yes: true,
            };
            let args = call_args(&options, &arm_dir, arm, scenario).unwrap();
            let system =
                std::fs::read_to_string(arm_dir.join("calls/package-manager/system.md")).unwrap();
            assert_eq!(
                system.contains("use-npm-install"),
                arm == Arm::File,
                "{system}"
            );
            assert!(args.contains(&"--strict-mcp-config".to_string()));
        }
    }
}
