# Measuring it: the built-in A/B

> Pre-release: `graphersal-memory bench` is planned for stage M5. This page describes its method,
> and the numbers from the hand-made measurement it is based on.

Every memory tool claims to help. `bench` lets you check whether one does on **your** memory, with
**your** harness, before you rely on it.

## Method

### Arms

Each arm answers the same questions:

- **No memory**: shows what any memory adds at all.
- **As is**: a copy of your current setup (MCP servers, hooks, plugins, memory files), minus
  graphersal-memory's own entries. If you use another memory tool, it is part of this arm, so the
  comparison is that tool against ours.
- **graphersal-memory**: the same copy, without the other memory tool, plus graphersal-memory.
- **Variants** (develop mode): a changed memory structure on a copy of your store, to compare
  structures against each other. The winner is applied, and the rest is dropped.

### Sandboxing: your live configuration is never touched

A failed or interrupted benchmark must never leave your setup broken, so no arm switches anything
off or edits your configuration. Each arm is a sandbox built for the run:

- its own MCP configuration (for Claude Code: `--strict-mcp-config --mcp-config <file>`);
- its own settings file, with no project settings or hooks;
- an empty working directory, read-only tools only, and no session saved;
- graph variants on a clone or a snapshot fork of the store, never on the live store.

Other memory tools are detected from a known-tools list and heuristics, and you confirm them. In
their arm, they run on their own data, read only.

### Questions and references

- Questions are generated from your own sessions and memories, in three categories: **rules**,
  **decisions** (with their why) and **history** (what happened in past sessions).
- **Reference answers come from your raw transcripts**, which every arm shares, never from
  graphersal-memory's own summaries: references written from the summaries one arm serves favour
  that arm. You confirm a sample of the references before the run.

### Running

- **Per-run id.** Every question starts with a fresh run id. Otherwise a repeated run reuses the
  cached tail of identical prompts, and its cost looks lower than a real first-time question.
- **Warm-up** per arm, so every arm starts with the same prompt-cache state.
- **Repeats**, to show the spread between runs.
- **A cost estimate before anything is spent**, and a dry-run mode that calls no model.

### Judge and metrics

A separate model call scores each answer from 0 to 5 against the reference and flags **confidently
wrong** answers: wrong facts stated as true, which are worse than "I don't know". The report gives,
per arm and per category:

- mean score, and the number of confidently wrong answers;
- mean cost (list price), turns, wall time, input tokens;
- **cost per score point**: mean cost divided by mean score.

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
