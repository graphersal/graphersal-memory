# Measuring it: the built-in A/B

> `graphersal-memory bench`, through Claude Code, opencode, or Codex with a local model
> (`bench rule-changes`: Claude Code). The arm with your whole current setup and the structure
> variants of develop mode are planned. This page describes the
> method, a first run of the built-in bench, and the hand-made measurement it is based on.

Every memory tool claims to help. `bench` lets you check whether one does on **your** memory, with
**your** harness, before you rely on it.

## Running it

```sh
graphersal-memory bench --dry-run              # plan, sandboxes, estimate; a fake harness, no spend
graphersal-memory bench                        # estimate, ask, generate questions if needed, run
graphersal-memory bench questions              # generate a new question set only (asks first)
graphersal-memory bench questions --review     # show the question set with its sources, confirm it
graphersal-memory bench questions add "Which database do we use?" --answer "SQLite"   # your own
graphersal-memory bench questions import my-questions.txt   # your own, from a file
graphersal-memory bench questions remove U2     # drop a question by its id
```

Options of `bench`: `--arms none,files,graph` (the default: all three), `--questions N` (12: how
many generated questions are asked besides your own, generated when the project has none yet;
`0` asks only your own), `--repeats R` (1), `--budget USD` (3), `--project
ROOT` (default: the git root of the current directory), `--claude-dir DIR` (read the memory
files, instruction files and transcripts from another Claude Code directory, for example a copy;
the harness keeps its own), `--yes` (do not ask), `--dry-run`, `--harness-command COMMAND`. The
harness command is `--harness-command`, else `summaries.harness_command` in `config.json`
(`claude -p --model sonnet` by default); `bench` drives Claude Code's `claude -p`, opencode's
`opencode run` and Codex's `codex exec --oss` with a local model. A command it cannot drive is
refused before anything runs, with the commands that work, ready to copy:

```sh
graphersal-memory bench --harness-command "claude -p"
```

Each run writes `report.md` and `report.json` to `bench/<timestamp>/` in the data directory, with
the sandboxes it used (the copy of the store is deleted after the run), and prints a summary. The
best arm of each row group (category, origin, all) is in bold in `report.md`, its name and its
mean score, and its line of the printed summary is green on a terminal that allows colour; a
tie of every arm marks none.

## Method

### Arms

Each arm answers the same questions:

- **none**: no memory, no instruction files, no tools. It shows what any memory adds at all.
- **files**: your file memory as it is: your `CLAUDE.md` files (the user one in the Claude Code
  directory, `CLAUDE.md` and `CLAUDE.local.md` in the project root; graphersal-memory's marked
  block left out) and `MEMORY.md` in the prompt, as Claude Code loads them, and the memory files
  readable with Read, Grep and Glob from a copy.
- **graph**: the same instruction files (they stay where your harness reads them with either
  memory), and graphersal-memory's read-only tools (`memories`, `memory`, `sessions`, `session`,
  `recall`) on a copy of your store.
- **current** (planned): a copy of your whole current setup (MCP servers, hooks, plugins, memory
  files), minus graphersal-memory's own entries, so that another memory tool you use is compared
  against ours. It is not built yet, because another tool's MCP server could write to its live
  data during the run; `--arms current` says so.
- **Variants** (planned, develop mode): a changed memory structure on a copy of your store, to
  compare structures against each other.

### Sandboxing: your live configuration is never touched

A failed or interrupted benchmark must never leave your setup broken, so no arm switches anything
off or edits your configuration. Each arm is a sandbox built for the run in
`bench/<timestamp>/arms/<arm>/`, and every call of `claude -p` gets:

- `--strict-mcp-config --mcp-config <arm>/mcp.json`: no MCP servers but the arm's (none, or for the
  graph arm `graphersal-memory mcp --no-daemon` on the copy of the store, never your live store
  and never your running daemon);
- `--setting-sources ""` and `--settings <arm>/settings.json`: none of your user, project or local
  settings, so none of your hooks, plugins or `CLAUDE.md` files; the arm's settings switch hooks
  off and deny the shell and write tools;
- `--tools` with the arm's read-only tools (none, `Read,Grep,Glob`, or none plus the graph's
  memory tools in `--allowedTools`), `--disable-slash-commands`;
- `--append-system-prompt-file <arm>/system.md`: the arm's context;
- an empty working directory (the files arm: the copy of the memory files),
  `--no-session-persistence` (no transcript of its own), `--max-budget-usd`, and
  `GRAPHERSAL_MEMORY_HOOK=off`.

The copy of the store is a fork of its latest state (`Store::fork_dir`), read without its lock, so
the bench works while your daemon runs. Tests build the sandboxes on a temporary home and check
that no file and no argument names the live configuration or store.

### Questions and references

- Questions are generated by your harness from your **raw transcripts**: the newest finished
  sessions of the project (sessions still being written are left out for 30 minutes), up to about
  120,000 characters, every turn marked with its id. The generator writes the asked number of
  questions in three categories, **rules**, **decisions** (with their why) and **history** (what
  happened in a session), each with a reference answer and the ids of the turns it comes from. A
  question without a source turn of the excerpt is dropped.
- **Reference answers never come from graphersal-memory's own summaries**: references written
  from the summaries one arm serves favour that arm.
- The set is kept per project in `bench/questions/<project>.json` with the start of each source
  turn beside it. You review it (`bench questions --review`): fix a reference, set `"keep":
  false` to drop a question, then confirm. `bench` asks before it uses an unconfirmed set;
  `--yes` accepts it, and the report then says it was not reviewed.

### Your own questions

Besides the generated questions you can add your own: the things you want the assistant to
remember. They are kept in the same file, marked `"origin": "user"` (ids `U1`, `U2`, ...; the
generated ones are `"generated"`, `Q1`, `Q2`, ...).

- **Adding.** `bench questions add "<question>" [--answer "<the right answer>"] [--category
  rules|decisions|history]` adds one; without a category it is reported as **other**, so a
  question you did not classify does not change the generated categories' numbers.
  `bench questions import FILE` adds the questions of a file: one per line, or `Q:` / `A:` pairs
  (the answer under its question; a line without a prefix continues the line before; lines
  starting with `#` are comments), or JSON (a list, or an object with `questions`, of entries
  like the question file's: `question`, optional `reference` or `answer`, optional `category`).
  It reports what it added and what it skipped; a question already in the set (the same words,
  ignoring case and punctuation) is skipped. `bench questions remove U2` drops one by its id;
  `"keep": false` in the file drops it as it drops a generated one.
- **Interactively.** When `bench` or `bench questions` runs in a terminal (without `--yes` and
  outside `--dry-run`), it first offers one optional step, "Add your own questions?": type a
  question, then its right answer or just Enter; an empty question line ends the step.
- **Mixing.** Your questions are always asked. `--questions N` is the number of generated
  questions asked besides them (the first N kept ones of the set); `--questions 0` asks only
  yours. Generating a new set (`bench questions`) replaces the generated questions and keeps
  yours; a generated question with the same words as one of yours is left out.
- **References.** With `--answer` (or an `A:` line, or a `reference` in JSON), your answer IS the
  reference (`"reference_from": "user"`). Without one, the next `bench questions` or `bench`
  asks your harness to find the answer in the same raw transcripts the generator reads (never
  graphersal-memory's summaries) and to name the turns it comes from; that call is part of the
  estimate shown before anything is spent. A found reference is reviewed like a generated one
  (`"reference_from": "transcripts"`, with its source turns). When the transcripts do not
  answer the question (`"reference_from": "not_found"`, `"reference": null`), it is still asked
  to every arm and its answers are shown in the report under "Questions without a reference",
  but it is **not judged and is in no score or mean**: judging without a reference could only
  reward a plausible guess. Write a reference into the file to have it scored, or set
  `"reference_from": "pending"` to have it looked up again.
- **The caveat of your own answers.** A question with your own answer is judged against your
  wording: a short note or an answer phrased differently can score differently than against a
  reference written from the transcripts. The judge is told the reference is yours and to compare
  facts, and the report counts these questions in its caveats.
- **The report** breaks the scores down by origin (your questions, generated questions) as well
  as by category; the headline mean holds every judged answer of both.
- A question file from before your own questions (format version 1) still loads: all its
  questions count as generated.

### Running

- **Per-run id.** Every question starts with the run id. Otherwise a repeated run reuses the
  cached tail of identical prompts, and its cost looks lower than a real first-time question.
- **Warm-up** per arm (a one-word call), so every arm starts with the same prompt-cache state.
- **Repeats**, to show the spread between runs (the report gives the mean score per repeat).
- The calls run one after another, arm by arm within each question.

### Judge and metrics

A separate call (the same harness, no tools) scores each answer from 0 to 5 against the reference
and flags **confidently wrong** answers: wrong facts stated as true, which are worse than "I don't
know". A failed answer or judging counts as 0 and is named in the caveats. An answer to a question
without a reference is not judged (see [Your own questions](#your-own-questions)). The report
gives, per arm and per category (and per origin when your own questions are in the run):

- mean score, and the number of confidently wrong answers;
- mean cost (list price, as `claude -p` reports it), input tokens (plain, cache writes and cache
  reads), output tokens, turns and wall time;
- **cost per score point**: mean cost divided by mean score;
- what the run spent on questions, warm-up, answers and judging, and its caveats.

### Cost guard

Before anything is spent, `bench` prints an estimate (questions x arms x repeats x about $0.05
per answer and $0.02 per judging, plus a warm-up per arm, the generation of the questions and the
lookup of the references of your own questions) and asks. A run whose estimate is above `--budget` is refused; every call carries a
`--max-budget-usd` cap; and the run stops, with a partial report, when its real spend would pass
the budget. `--dry-run` builds everything with a fake harness that calls no model.

### Rule changes (`bench rule-changes`)

A separate, smaller bench asks whether the next session follows a rule that changed. It uses
five built-in scenarios, never your memory: an old rule written in one session (npm, Redux,
Node 18, Heroku, 70 % coverage) and a newer one replacing it in the next. A question then needs
the rule, for example "What exact command do I run to install this project's dependencies?".

- **Arms.** Each arm has a store of its own in the run directory, one project per scenario, with
  eight unrelated memories beside the two rules. **plain** holds both memories with nothing
  linking them, as graphersal-memory was before it linked them. **marked** writes the newer one with
  `supersedes: [old]`, so every search marks the old one "SUPERSEDED by <new> on <date>:".
  **file** is marked, and the old rule also sits in a memory file loaded into the context: the
  case of a file nobody edited.
- **Calls.** Each call runs through your `claude -p` in a sandbox like the graph arm's
  (`mcp --no-daemon` over the arm's store, only the read tools memories/memory/recall, no
  settings, hooks or files of yours). There is no judge call: the answer's words decide whether
  it follows the new rule, the old one, both or neither (`pnpm` against `npm install`, for
  example).
- **Cost guard.** As for `bench`: the estimate comes first (5 scenarios x 3 arms x `--repeats`
  answers, about $0.05 each), a run above `--budget` is refused, every call is capped, and
  `--dry-run` calls no model. The report is `report.md` and `report.json` in
  `<data dir>/bench/<timestamp>-rule-changes/`.
- **Before it was built** (an experiment on 2026-11-12, Claude Haiku, 6 scenarios x 3 runs, by
  hand): 17/18 answers followed the new rule with plain memories, 18/18 marked, 13/18 when the
  old rule was also in a loaded file. Small samples, synthetic scenarios.

## A first run of the built-in bench

The first real run of `bench`, on 2026-11-10, made to check the command end to end, not to
measure the product. It ran on a temporary **copy** of one developer's memory (Graphersal's own
development): a fresh data directory, 64 memory files imported from a copy of the file memory,
and 10 of the project's transcripts copied and indexed (102 turns; **summaries off**, so the
graph had raw turns but no session facts). Command: `bench --questions 6 --repeats 1 --arms
none,files,graph`; answerer and judge `claude -p --model sonnet`; the 6 questions were generated
from 9 finished sessions and accepted with `--yes`, **not reviewed**.

| Category | Arm | Mean score (0-5) | Confidently wrong | Mean cost | Mean input tokens | Mean turns |
|---|---|---|---|---|---|---|
| rules (2) | none | 0.00 | 0 | $0.006 | 3,200 | 1.0 |
| rules (2) | files | 4.50 | 0 | $0.019 | 64,123 | 3.0 |
| rules (2) | graph | 5.00 | 0 | $0.022 | 46,090 | 2.5 |
| decisions (3) | none | 0.00 | 0 | $0.006 | 3,206 | 1.0 |
| decisions (3) | files | 1.00 | 0 | $0.017 | 59,691 | 2.3 |
| decisions (3) | graph | 3.33 | 0 | $0.064 | 167,336 | 7.7 |
| history (1) | none | 0.00 | 0 | $0.005 | 3,200 | 1.0 |
| history (1) | files | 0.00 | 0 | $0.017 | 51,242 | 3.0 |
| history (1) | graph | 1.00 | 0 | $0.041 | 96,673 | 6.0 |
| **all (6)** | none | **0.00** | 0 | $0.006 | 3,203 | 1.0 |
| **all (6)** | files | **2.00** | 0 | $0.018 | 59,760 | 2.7 |
| **all (6)** | graph | **3.50** | 0 | $0.046 | 115,144 | 5.7 |

Cost per score point: files $0.009, graph $0.013. The run spent $0.91 at list price (questions
$0.17, warm-up $0.19, answers $0.42, judge $0.14); a first attempt spent $0.17 on questions that
were all dropped (the generator cited turns without their `turn:` prefix; `bench` now accepts
that).

Caveats:

- **Tiny sample**: 6 questions, one repeat, one project, one model; one question more or less
  moves a mean by most of a point. This says the command works, not how much memory helps.
- **The graph answer cost more here**: 2.5x the cost and about 2x the input tokens of a file
  answer, because the graph arm searched raw turns in many steps (up to 10 turns per answer)
  without session summaries. It was more expensive per score point too, unlike the prototype
  measurement below, which had summaries and facts.
- **The questions were not reviewed**; an LLM wrote them and their references from raw turns,
  and the same model judged.
- **Instruction files**: both memory arms carried the project's large `CLAUDE.md` files, which
  is most of their input tokens.
- **No memory scored 0** on every question, as it should: nothing of the user's leaked into that
  arm.

## Our first measurement

This is our own measurement on one developer's real memory (n=21 questions), made by hand on
2026-11-07 with the prototype graphersal-memory is built from: Graphersal's own development memory.
Both arms got the same instruction. The **files** arm had the `MEMORY.md` index in the prompt and
read/search tools over the memory files. The **graph** arm had a bounded index in the prompt and
read-only memory tools over MCP. Answerer and judge: `claude -p --model sonnet`.

| Category | Arm | Mean score (0-5) | Confidently wrong | Mean cost $ | Mean turns | Mean input tokens |
|---|---|---|---|---|---|---|
| rules (7) | files | 4.00 | 0 | 0.019 | 1.3 | 15,772 |
| rules (7) | graph | 5.00 | 0 | 0.025 | 2.1 | 29,678 |
| decisions (6) | files | 3.00 | 0 | 0.026 | 2.2 | 24,996 |
| decisions (6) | graph | 4.67 | 0 | 0.033 | 2.7 | 33,755 |
| history (8) | files | 0.00 | 0 | 0.026 | 2.2 | 26,627 |
| history (8) | graph | 4.00 | 0 | 0.041 | 3.4 | 39,120 |
| **all (21)** | files | **2.19** | 0 | 0.024 | 1.9 | 22,543 |
| **all (21)** | graph | **4.52** | 0 | 0.034 | 2.8 | 34,440 |

Cost per score point: files $0.024 / 2.19 = **$0.011**, graph $0.034 / 4.52 = **$0.0075**. The
whole run (answers and judge) cost $1.98 at list price.

### Caveats

- **Small sample.** It is one developer, one project and 21 questions. One model answered, and
  the same model judged. There was one run per arm in this configuration.
- **A graph answer costs more.** It costs more in dollars ($0.034 against $0.024) and uses about
  1.5x the input tokens. It wins on cost per point of quality, not on cost per answer.
- **History is structurally lopsided.** File memory has no session history, so it scored 0 there.
  Part of the history references were written from the same LLM session summaries that the graph
  serves, which favours the graph. The product's `bench` avoids this with references from raw
  transcripts.
- **The best configuration is reported.** Earlier configurations of the graph arm on the same
  questions scored 4.19 (all 21) and 4.33 (with the full file index in the prompt instead of the
  bounded one). The configuration reported above adds facts extracted from session summaries.

Source: our internal run reports of 2026-11-07, one per configuration. They are not published,
because the questions and answers come from a private development memory. The product's `bench`
writes the same kind of report for your own data.
