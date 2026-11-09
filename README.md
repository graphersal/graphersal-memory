# graphersal-memory

**Local graph memory for AI coding agents: your agent's rules, decisions and past sessions in a
graph on your machine, searched through MCP instead of loaded on every turn.**

> [!WARNING]
> **Pre-release. Nothing in this repository works yet.** This README describes the product we are
> building, and the [status table](#status) says what exists. Today the binary has its command set and
> help text, and every command says which stage will deliver it. Don't install it expecting memory
> yet. Star or watch the repository if you want to follow along.

## The problem

Coding agents forget. Every new session starts from scratch, so you repeat yourself:

- **Rules get lost.** "Never push without asking", "new tests are fine, changing an existing test
  needs approval". You write them down, but a rule buried among many others is easy to miss.
- **Decisions lose their why.** The agent sees *what* the code does, but not why you chose it over
  the alternative you rejected last month.
- **History is gone.** What happened in last week's session, which bug you found, what you agreed
  on: file memory doesn't record this at all.
- **File memory grows, and every turn pays for it.** A `MEMORY.md`-style index is loaded on every
  turn, so the more you remember, the more context each turn costs, and the harder it is for the
  agent to find the one entry that matters.

## What graphersal-memory does

- **One local graph for your memory.** Rules, decisions, preferences, facts, and every past session,
  connected to each other and to the project they belong to. It is stored in your per-user data
  directory and never leaves your machine.
- **Search through MCP.** The agent gets a few small tools: search memories, read one, remember,
  forget, search past sessions, read a session. It fetches the few entries that answer the question
  instead of reading everything.
- **Past sessions become searchable.** Finished sessions of your harness are indexed: raw turns
  always, and short summaries with decisions, corrections, outcomes and open threads if you opt in
  (summaries run through your own harness, see [Privacy](#privacy)).
- **Rules go back to where your harness reads them.** Rules kept in the graph are exported into your
  harness's native files, such as a marked block in `CLAUDE.md`. Only that marked block is written.
  Your own text is never touched.
- **Two modes, two MCP servers.**
  - `use` (on): the everyday memory tools.
  - `develop` (off by default, so it costs no context): tools to improve the memory's structure,
    measure, try a change on a copy, and keep it or roll it back.

  The engine enforces what each server may do, not just the tool list.
- **A built-in A/B** so you can check on your own data that it helps (see below).

### What it looks like (illustration)

> *Illustration of the intended behaviour, written by hand. It is not output of the current build,
> and tool names may change.*

```text
you   > Why don't we run the test suite on macOS in CI any more?

agent > [calls sessions("macOS removed from CI test matrix", k=3)]
        -> fact (decision, 2026-10-02, session 3f9c...): CI is manual-only (workflow_dispatch);
           macOS runners cost 10x the Linux minutes on the free plan, and macOS is tested
           locally before every release. Release wheels for macOS are still built.

agent > We dropped macOS from the CI test matrix on 2 October. macOS runners cost ten times the
        Linux minutes on the free plan, and you test on macOS locally before each release. CI
        became manual-only at the same time. macOS release builds are still produced. Do you want
        me to re-enable it for this branch?
```

## Install in one line

> *Target commands. They do not work yet: binaries and install scripts arrive in stage M6.*

macOS and Linux:

```sh
curl -fsSL https://github.com/graphersal/graphersal-memory/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/graphersal/graphersal-memory/releases/latest/download/install.ps1 | iex
```

With Rust installed, on any platform:

```sh
cargo install --locked --git https://github.com/graphersal/graphersal-memory graphersal-memory
```

Then, once:

```sh
graphersal-memory install
```

### What `install` changes, and how `uninstall` reverts it

`graphersal-memory install` is a short wizard that shows you every step first. For Claude Code it
will:

1. **Register the MCP server(s)** in Claude Code: `use` is on, and `develop` stays off until you
   enable it. It uses `claude mcp add` when available. Otherwise it edits the JSON config carefully
   and keeps a backup.
2. **Add two hooks.** At session start, a quick sync and background indexing of finished sessions.
   At session end, an export. Neither blocks your session.
3. **Import your existing memory**: memory files, `MEMORY.md`, and rules from `CLAUDE.md`. It reads
   them and does not delete them.
4. **Write the managed rule block**, a clearly marked section in your instruction file, telling the
   agent where its memory lives.
5. **Optionally download a better embedding model** (potion-multilingual-128M). Without it, a
   built-in embedder works with no download.
6. **Print a report**: what was imported, what was changed, where the backups are.

`graphersal-memory uninstall` removes exactly what `install` added: the MCP entries, the hooks and
the marked block. Anything you wrote yourself stays as it is. The acceptance test for this is
byte-for-byte: install, then uninstall, on a temporary home directory must leave the configuration
identical. `status` and `doctor` show what is installed and how to fix what isn't working.

No ports, no tokens, no system service. The harness starts `graphersal-memory mcp`, which starts a
small background daemon on demand. The daemon exits after two idle hours (configurable). See
[How it works](docs/how-it-works.md).

## Does it help? Measure it

We don't ask you to take our word for it. `graphersal-memory bench` will run an A/B on **your own**
memory and sessions:

- **Arms**: no memory; your current setup as it is (including another memory tool, if you use
  one); and graphersal-memory.
- **Sandboxed**: every arm runs in a sandbox built for the run. Your live configuration is never
  switched off or changed.
- **References from raw transcripts**: questions and reference answers come from your raw session
  transcripts, which every arm shares. They never come from our own summaries, which would favour
  us. You confirm a sample of them.
- **Judged and reported**: a judge scores each answer from 0 to 5 and flags confidently wrong
  answers. The report gives quality, cost, tokens and cost per score point.

Method: [docs/ab-testing.md](docs/ab-testing.md).

### What we measured so far

This is our own measurement on one developer's real memory (n=21 questions), made by hand with the
prototype this product is built from (2026-11-07). File memory (`MEMORY.md` + memory files) is
compared with graph memory. The answerer and the judge were both `claude -p --model sonnet`.

| Questions | File memory, score 0-5 | Graph memory, score 0-5 |
|---|---|---|
| Rules (7) | 4.00 | 5.00 |
| Decisions (6) | 3.00 | 4.67 |
| History of past sessions (8) | 0.00 | 4.00 |
| **All (21)** | **2.19** | **4.52** |

| Per answer | File memory | Graph memory |
|---|---|---|
| Confidently wrong answers | 0 | 0 |
| Mean cost (list price) | $0.024 | $0.034 |
| Mean input tokens | 22,543 | 34,440 |
| Cost per score point | $0.011 | $0.0075 |

**Read this with the caveats:**

- **Small sample.** It is one developer, one project, 21 questions, one model. The judge is an LLM.
- **The graph answer costs more.** At $0.034 against $0.024 and about 1.5x the input tokens, a
  graph answer costs more. It is cheaper only per point of quality.
- **History favours the graph by construction.** File memory has no session history at all, so it
  scores 0 there. Some history references were written from the same LLM session summaries that
  the graph serves, which favours the graph. The product's `bench` builds references from raw
  transcripts for this reason.
- **The best-scoring configuration is reported.** Earlier configurations of the graph arm on the
  same questions scored 4.19 and 4.33.

That is why the A/B ships with the product: measure it on your own data before you rely on it.
Sources and the full method are in [docs/ab-testing.md](docs/ab-testing.md#our-first-measurement).

## Privacy

- **Everything stays local.** The memory store lives in your per-user data directory, and the
  daemon listens on a local socket that only your OS user can open. There are no network ports.
- **No telemetry.** Nothing is reported anywhere.
- **Summaries are opt-in.** Session summaries run only through your own harness (for example
  `claude -p` on your own subscription), never through an API key of ours or a service of ours.
  They run only after you agree, with a cost estimate first and a monthly budget cap. Above the
  cap, only raw turns are indexed, and summaries catch up later.
- **The network is used only when you ask.** That covers the optional model download and
  installing or updating the binary.

Details: [docs/privacy.md](docs/privacy.md).

## Supported harnesses

| Harness | Status |
|---|---|
| Claude Code | first target (planned, stage M3) |
| opencode | planned (M7) |
| OpenAI Codex CLI | planned (M7) |
| Cursor | planned (M7) |
| Desktop apps without a terminal (e.g. Claude Desktop) | later; the core is designed not to depend on a terminal or hooks |

Details: [docs/harnesses.md](docs/harnesses.md).

## Status

| Feature | Status |
|---|---|
| Command-line skeleton (`--help`, command set) | available (every command reports "not implemented yet") |
| Memory core: store, memory tools, MCP over stdio | planned (M1) |
| Shared daemon + stdio shim, several sessions at once | planned (M2) |
| One-command install / uninstall for Claude Code, import, rule export | planned (M3) |
| Indexing past sessions, opt-in summaries and facts | planned (M4) |
| Built-in A/B (`bench`) | planned (M5) |
| Release binaries and one-line installers | planned (M6) |
| Other harnesses, browser UI, desktop app | planned (M7) |

The roadmap in user-facing words: [docs/roadmap.md](docs/roadmap.md). Why this exists:
[docs/why.md](docs/why.md).

## FAQ

**Does it replace `CLAUDE.md`?** No. Rules stay where your harness reads them, and
graphersal-memory writes only a marked block. The graph adds search over everything that
shouldn't be loaded on every turn.

**Can I go back?** `uninstall` removes what `install` added. Your own files are imported, not
deleted.

**Does it need a GPU, a cloud account or Docker?** No. It is one binary with a built-in embedder.

**I already use another memory tool.** Keep it. The A/B can compare it against graphersal-memory
without changing your setup. Import from another MCP memory tool goes through the agent itself:
both servers are connected, the agent reads from one and writes to the other.

## Built on Graphersal

graphersal-memory is built on [Graphersal](https://github.com/graphersal/graphersal), an embeddable
graph database and traversal engine in Rust with GQL and Gremlin, vector search, durable storage
and an MCP endpoint ([site](https://graphersal.github.io)). graphersal-memory is its first one-click
product and uses only its public API.

## Licence

Licensed under the [Apache License, Version 2.0](LICENSE). See [NOTICE](NOTICE).
