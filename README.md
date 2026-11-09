# graphersal-memory

**Local graph memory for AI coding agents: your agent's rules, decisions and past sessions in a
graph on your machine, searched through MCP instead of loaded on every turn.**

> [!WARNING]
> **Pre-release. The memory core, the shared daemon, the one-command install for Claude Code and
> the indexing of past sessions (with opt-in summaries) work so far.** This README describes the
> product we are building, and the [status table](#status) says what exists. Today you build the
> binary with Rust and run `graphersal-memory install` ([try it](#try-it)); release binaries and
> the built-in A/B are still to come, and every command that is not built yet says which stage
> delivers it. Star or watch the repository if you want to
> follow along.

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

> *The binaries and install scripts arrive in stage M6, so the two one-liners below do not work
> yet. The `cargo install` line works today.*

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

`graphersal-memory install` is a short wizard that shows you every step first and asks before it
changes anything (`--dry-run` only shows the plan, `--yes` skips the question). For Claude Code it:

1. **Registers the MCP server** `graphersal-memory` (the `use` tools) at user scope in
   `~/.claude.json`. The `develop` server stays off: Claude Code cannot switch a user-scope server
   off, so it is simply not registered until you run `install --enable-develop`
   (`uninstall --disable-develop` removes it again).
2. **Adds two hooks** to `~/.claude/settings.json`. At session start and at session end, the
   memory files of the current project that are new or changed are imported, the indexing of
   finished sessions is queued in the background daemon, and the session start adds one status
   line to the agent's context. Neither blocks your session: each stops after 15 seconds and
   never fails, and the indexing itself never runs in the hook.
3. **Imports your existing file memory**: `MEMORY.md` and the memory files of every project under
   `~/.claude/projects/`, with their kind, title, order and `[[links]]`, into the project they
   belong to. It reads them and does not change or delete them, and running it again updates
   instead of duplicating. Rules in `CLAUDE.md` files are not imported yet.
4. **Writes the managed rule block**, a clearly marked section in `~/.claude/CLAUDE.md`, telling the
   agent that its memory lives in graphersal-memory and how to use the tools. If you edit inside
   the block, install and uninstall report a conflict and leave your edit alone.
5. **Indexes your past sessions** in the background: every finished transcript under
   `~/.claude/projects/` becomes a searchable session with its turns, locally and for free.
   **Summaries are asked for separately and default to no**: install shows the number of
   sessions and an estimated cost, and only a "yes" (or `--summaries`) switches them on; `--yes`
   alone leaves them off. See [Privacy](#privacy).
6. **The better embedding model** (potion-multilingual-128M) is planned: `--with-model` says so
   today, and the built-in embedder works with no download.
7. **Prints a report**: what was imported, which files changed, where the backups are.

The JSON files are edited in place: only our entries are inserted, the rest of each file keeps its
bytes, and every file is backed up into the data directory before its first change. (`claude mcp
add` is not used, because it rewrites the whole file.)

`graphersal-memory uninstall` removes exactly what `install` added: the MCP entries, the hooks and
the marked block. A file nobody changed since install gets its original bytes back (or is deleted
again if install created it); in a file Claude Code rewrote since, only our entries are removed.
Anything you wrote yourself stays as it is. The acceptance test for this is byte-for-byte:
install, then uninstall, on a temporary home directory leaves the configuration identical.
Your memories stay in the data directory; `uninstall --purge` deletes them too (after asking).
`status` shows what is installed, and `doctor` checks it and says how to fix what isn't working.

No ports, no tokens, no system service. The harness starts `graphersal-memory mcp`, which starts a
small background daemon on demand. The daemon exits after two idle hours (configurable in
`config.json` in the data directory). See [How it works](docs/how-it-works.md).

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
| Claude Code | install, uninstall, import of the file memory, hooks, rule block (M3); past sessions and opt-in summaries (M4) |
| opencode | planned (M7) |
| OpenAI Codex CLI | planned (M7) |
| Cursor | planned (M7) |
| Desktop apps without a terminal (e.g. Claude Desktop) | later; the core is designed not to depend on a terminal or hooks |

Details: [docs/harnesses.md](docs/harnesses.md).

## Status

| Feature | Status |
|---|---|
| Command-line skeleton (`--help`, command set) | available (commands of later stages report "not implemented yet": `ui`, `bench`, `export`) |
| Memory core: store, memory tools, project and global memories, `use`/`develop` profiles, MCP over stdio | available (M1) |
| Shared daemon + stdio shim, several sessions at once (`mcp`, `daemon`, the daemon part of `status`) | available (M2; Linux and macOS tested, Windows builds but is untested) |
| One-command install / uninstall for Claude Code, `status`, `doctor`, import of the file memory (`import`), the managed rule block, the session hooks, `config.json` | available (M3; Linux and macOS tested, Windows builds and its unit tests run) |
| Import of `CLAUDE.md` rules, export of rules kept in the graph (`export`), the model download | planned |
| Indexing past sessions (`import --sessions`, the hooks), opt-in summaries and facts through your own harness (`summaries estimate`, `status`, `enable`, `disable`), cost estimate and monthly budget cap | available (M4; tested with a fake harness, no model was called in the tests) |
| Built-in A/B (`bench`) | planned (M5) |
| Release binaries and one-line installers | planned (M6) |
| Other harnesses, browser UI, desktop app | planned (M7) |

### Try it

Build the binary with Rust (`cargo install --locked --git
https://github.com/graphersal/graphersal-memory graphersal-memory`), then set it up for Claude
Code:

```sh
graphersal-memory install --dry-run   # the plan, nothing changes
graphersal-memory install             # shows the plan again and asks
graphersal-memory status              # what is installed, whether the daemon runs
```

Restart your Claude Code sessions afterwards. Details:

- Every agent session starts its own `graphersal-memory mcp`. The first one starts a background
  daemon that owns the store; every session talks to it over a local socket, so several sessions
  share one memory at once and see each other's writes. The daemon exits after two hours without
  a session (`"daemon": {"idle": "30m"}` in `config.json`, or `GRAPHERSAL_MEMORY_IDLE=30m`), and the
  next session starts it again. Its log is `daemon.log` in the run directory.
- The store is created on first use in your per-user data directory (macOS
  `~/Library/Application Support/graphersal-memory`, Linux `~/.local/share/graphersal-memory`,
  Windows `%LOCALAPPDATA%\graphersal-memory`). `--data-dir DIR` or the environment variable
  `GRAPHERSAL_MEMORY_DIR` puts it elsewhere, for example to try it on a throwaway store (such a
  data directory gets a daemon of its own; `install` with it registers the server for that
  directory).
- The project is the git repository root of the directory the harness starts it in (or that
  directory); `--project ROOT` sets it. Memories are stored for the project unless the agent
  saves them as global, and searches see the project's memories plus the global ones. The project's
  git remote is recorded too, so that a later version can follow a moved repository.
- `--profile use` (the default) offers the memory tools: `memories`, `memory`, `remember`,
  `forget`, `sessions`, `session`, `recall`. `--profile develop` adds schema, query, GQL and
  saved-query tools (`install --enable-develop` registers it as `graphersal-memory-develop`).
- `mcp --no-daemon` serves the store inside that one process instead (one session at a time: a
  second session fails to open the store, and so does a daemon while it runs).
- `graphersal-memory import` imports new and changed memory files at any time (the hooks do it for
  the current project).
- Past sessions: finished Claude Code transcripts (unchanged for 30 minutes, or ended) are
  indexed by the daemon in the background; the hooks queue it, and `graphersal-memory import
  --sessions` runs it now with progress. Without summaries, `sessions()` lists the newest
  sessions and `recall(text, label="turn")` searches what was said; `sessions(text)` searches the
  facts of summaries, so it finds something once summaries are on.
- Summaries: `graphersal-memory summaries estimate` shows the sessions waiting and an estimated
  cost, `summaries enable` switches them on (it asks; `--budget 5` sets the monthly cap),
  `summaries status` shows the month's spend, `summaries disable` switches them off. They run
  through `claude -p --model sonnet` on your own account, or the command in `config.json`
  (`summaries.harness_command`).
- To register it by hand instead: `claude mcp add -s user graphersal-memory -- graphersal-memory
  mcp`.

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
