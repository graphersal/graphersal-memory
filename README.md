# graphersal-memory

**Local graph memory for AI coding agents: your agent's rules, decisions and past sessions in a
graph on your machine, searched through MCP instead of loaded on every turn.**

> [!WARNING]
> **Pre-release. The memory core, the shared daemon, the one-command install for Claude Code, the
> indexing of past sessions (with opt-in summaries) and the built-in A/B work so far.** This
> README describes the product we are building, and the [status table](#status) says what
> exists. Today you build the binary with Rust and run `graphersal-memory install`
> ([try it](#try-it)); release binaries and the one-line installers are built and arrive with the
> first published release, and every command that is not built yet says which stage delivers it.
> Star or watch the repository if you want to follow along.

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

> *The two one-liners work once the first release is published (the release workflow and the
> installers exist; the repository is not public yet, and GitHub serves release files of a
> private repository to signed-in members only). The `cargo install` line works today for
> anyone with access to the repository.*

macOS and Linux:

```sh
curl -fsSL https://github.com/graphersal/graphersal-memory/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/graphersal/graphersal-memory/releases/latest/download/install.ps1 | iex
```

The installers pick the archive of your platform (macOS arm64 and x86_64, Linux x86_64 and
aarch64 as static binaries, Windows x86_64), check it against the release's `SHA256SUMS`, and put
the binary into `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\graphersal-memory`). They change
nothing else, not even your `PATH` (they tell you when it lacks the directory). A given version or
directory: `... | sh -s -- --version v0.1.0 --dir /usr/local/bin`; on Windows
`& ([scriptblock]::Create((irm <url of install.ps1>))) -Version v0.1.0 -Dir C:\Tools`. Rather check
first? Download the archive from the [releases](https://github.com/graphersal/graphersal-memory/releases),
compare it with `SHA256SUMS` (and, for a public release, its build provenance:
`gh attestation verify <archive> -R graphersal/graphersal-memory`), and put `graphersal-memory` on
your `PATH` yourself.

With Rust installed, on any platform:

```sh
cargo install --locked --git https://github.com/graphersal/graphersal-memory graphersal-memory
```

Then, once:

```sh
graphersal-memory install
```

### What `install` asks, changes, and how `uninstall` reverts it

`install` sets up every agent harness it finds: Claude Code, opencode, OpenAI Codex CLI and
Cursor (`--harness claude-code|opencode|codex|cursor|all` chooses). A harness counts as found
when its program is (on `PATH`, or where its installer puts it: `~/.local/bin/claude`,
`~/.claude/local/claude`, `~/.opencode/bin/opencode`, Cursor's app); a configuration directory
left behind without the program is reported, not offered (`--harness` installs there anyway,
with a warning); with no agent program found at all, install changes nothing and says so. With more than one found, the wizard's first step asks which; every chosen agent gets the same memory (one store, one daemon). For opencode
(1.x and 2.x; install asks `opencode --version` which one it is) it registers the MCP servers in
`~/.config/opencode/opencode.json`, a rules file listed in `instructions` and a small session
plugin; for Codex, the MCP servers in `~/.codex/config.toml`, the rule block in
`~/.codex/AGENTS.md` and two hooks in `~/.codex/hooks.json` (Codex runs them once you trust them
in `/hooks`); for Cursor, the MCP server in `~/.cursor/mcp.json`, a rule file of its own in
`~/.cursor/rules/` and two hooks in `~/.cursor/hooks.json` (Cursor support is checked against
its documentation and fixtures; a live check is pending). For these three, `uninstall` exports
the memories as Markdown (none of them has a file memory).
The details are in [docs/harnesses.md](docs/harnesses.md), the captured wizards in
[docs/install.md](docs/install.md). The Claude Code steps:

In a terminal, `graphersal-memory install` is a short wizard: "Step N of M", what the step is
for, numbered choices with the default marked (Enter takes it), and a summary of every change
before anything happens. The full captured output of an install and an uninstall is in
[docs/install.md](docs/install.md). The steps:

1. **Memory language.** English only (the default: the agent stores and searches in English and
   translates a query first, so a memory is found whatever language you wrote in) or any
   language (the agent stores in the language of the conversation).
2. **Embedding model**, which turns every memory into a vector so a search finds related
   wording, not only the same words. Offered: `minishlab/potion-retrieval-32M` (English, about
   130 MB, MIT; the default for English), `minishlab/potion-multilingual-128M` (100+ languages,
   about 530 MB, MIT; the default for any language), or the built-in embedder (no download,
   works offline, but finds only shared words and spellings, not meaning). The download from
   Hugging Face is the one network access of install; it is checked (SHA-256), kept in the data
   directory, and the daemon then re-embeds your memory with it in the background. If the
   download fails, the built-in embedder stays and install goes on.
3. **Import your existing file memory**: `MEMORY.md` and the memory files of every project
   under `~/.claude/projects/`, with their kind, title, order and `[[links]]`. Every memory stays
   in its project, as Claude Code keeps it. Install reads the files and never changes them.
4. **Switch off Claude Code's own auto-memory** (default yes): `autoMemoryEnabled: false` in
   `~/.claude/settings.json`, so the same facts are not kept twice. Nothing is lost: if you
   uninstall graphersal-memory later, every memory, including new ones, is written back to
   Claude Code's memory files and its auto-memory is switched back on.
   **Other memory** (only when found: another memory tool such as mem0, Basic Memory or the
   reference memory server, an instruction line about memory elsewhere): graphersal-memory becomes
   the main memory and the other tool read-only (default), or you switch it off (uninstall switches
   it on again), or keep both; its memories can be imported. See
   [harnesses.md](docs/harnesses.md#other-memory-beside-graphersal-memory).
5. **Session summaries** (no default: you choose). Past sessions are always indexed locally from
   their raw turns. Summaries go through your own `claude -p`, so the step shows the number of
   sessions and an estimated cost; see [Privacy](#privacy).
6. **The develop server** (no default): a second MCP server with structure tools. Claude Code
   cannot switch a user-scope server off, so "no" means it is not registered.
7. **Summary and confirm.** Enter applies.

What it then writes: the MCP server `graphersal-memory` at user scope in `~/.claude.json`; two
hooks in `~/.claude/settings.json` (at session start and end the current project's new or changed
memory files are imported and the indexing of finished sessions is queued; neither blocks the
session); the managed rule block, a marked section in `~/.claude/CLAUDE.md` with the memory
language rule (an edit inside it is reported as a conflict and left alone); and `config.json` in
the data directory. Then it prints a report.

<details>
<summary>The wizard, as captured (a pseudo-terminal on a throwaway home; answers: Enter, Enter, Enter, Enter, 2, 2, Enter)</summary>

```text
graphersal-memory install (Claude Code)
A few questions; Enter takes the default. Nothing changes before the last step.

Step 1 of 7 · Memory language
  Memories are found by meaning. Keeping them in one language lets a search find a
  memory whatever language you wrote in; or keep each memory in your own words.

  1) English only  the agent stores and searches in English  (default)
                   (it translates a query before searching)
  2) Any language  the agent stores in the language of the conversation
                   (and searches in it and in English)
Choose [1]: 

Step 2 of 7 · Embedding model
  The model turns every memory into a vector, so a search finds related wording,
  not only the same words. Downloaded once from Hugging Face; it then runs locally.

  1) potion-retrieval-32M      English, ~130 MB, MIT  (default)
  2) potion-multilingual-128M  100+ languages, ~530 MB, MIT
  3) Built-in (no download)    works offline, but finds only shared words and spellings,
                               not meaning; weaker search
Choose [1]: 

Step 3 of 7 · Import existing file memory
  2 memory files of Claude Code's own memory (MEMORY.md and the files beside
  it) in 1 project are not in the graph yet. They stay per project, as
  Claude Code keeps them. The files are read, never changed.

  1) Yes  import them now  (default)
  2) No   not now (the session start hook still imports the files of the
          project you work in)
Choose [1]: 

Step 4 of 7 · Claude Code's auto-memory
  Claude Code also writes its own memory files and loads them into every session;
  with graphersal-memory both would keep the same facts twice.
  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new
  ones, is written back to Claude Code's memory files and its auto-memory is switched
  back on.

  1) Yes  switch it off (autoMemoryEnabled false in /tmp/gmw/h/.claude/settings.json)  (default)
  2) No   keep it on (the files it writes are imported at each session start)
Choose [1]: 

Step 5 of 7 · Session summaries
  Past sessions are indexed locally (their raw turns, no model call). Optionally each
  finished session is summarized through your own `claude -p --model sonnet`
  (decisions, corrections, outcomes, open threads). That costs model usage:
  0 finished sessions, 0 without a summary: about $0.00 at list price (an estimate: ~4
  characters per token, $3/$15 per million input/output tokens, plus the harness's own
  prompt; your harness reports the real cost)

  1) Yes  summarize, at most $5.00 per month
  2) No   raw turns only (`graphersal-memory summaries enable` later)
Choose 1-2: 2

Step 6 of 7 · The develop server
  A second MCP server with tools to inspect and reshape the memory's structure
  (schema, saved queries, indexes). Most people do not need it; its tool
  descriptions are paid on every agent turn.

  1) Yes  register graphersal-memory-develop
  2) No   not now (`graphersal-memory install --enable-develop` later)
Choose 1-2: 2

Step 7 of 7 · Ready to install

  Agents: Claude Code
  Memory language: English only (stored and searched in English)
  Embedding model: minishlab/potion-retrieval-32M (~130 MB download, MIT)
  File memory: 2 memory files imported now
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.

Install? [Y/n, d = show every change]: 

Embedding model:
  Downloading minishlab/potion-retrieval-32M from https://huggingface.co
    config.json: 202 B / 202 B (100%)
    tokenizer.json: 1.4 MiB / 1.4 MiB (100%)
    model.safetensors: 123.2 MiB / 123.2 MiB (100%)
    README.md: 16.7 KiB / 16.7 KiB (100%)

Report:
  Backups of the files changed: /tmp/gmw/d/install/backups/2026-11-10T072304Z/

Claude Code:
  created ~/.claude.json
  changed ~/.claude/settings.json (backed up)
  created ~/.claude/CLAUDE.md
  Claude Code's auto-memory: off (uninstall writes every memory back to its memory files and switches it on again)

Memory:
  created /tmp/gmw/d/config.json
  embedding model: downloaded minishlab/potion-retrieval-32M (124.6 MiB, licence MIT, model2vec/minishlab/potion-retrieval-32M@c7f8727ce247) into /tmp/gmw/d/models/minishlab/potion-retrieval-32M
  embedding model: minishlab/potion-retrieval-32M (the daemon re-embeds the memory with it in the background)
  imported 2 memories (2 new, 0 updated) into 1 project; 0 unchanged
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexing in the background (`graphersal-memory import --sessions` shows the progress)

----------------------------------------
Next steps
  1. Restart running Claude Code sessions to load the MCP server and hooks.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

</details>

Every question has a flag for scripts: `--language english|any`, `--model <repo>|builtin`,
`--no-import`, `--keep-auto-memory`, `--summaries`, `--enable-develop`, `--other-read-only NAME`,
`--keep-other NAME`, `--disable-other NAME`, `--enable-other NAME`. With `--yes`, or without a
terminal, every question takes its default (summaries and the develop server stay off unless
their flag is given); without `--yes` it still asks once before applying (and, with nothing to
answer, such as a pipe, says so and changes nothing). The wizard's last step is a short recap;
`d` there, or `--dry-run`, shows every change, grouped by agent. `--dry-run` changes nothing. Running `install` again shows your current answers as the defaults and
changes only what differs.

The JSON files are edited in place: only our entries are inserted, the rest of each file keeps its
bytes, and every file is backed up into the data directory before its first change. (`claude mcp
add` is not used, because it rewrites the whole file.)

`graphersal-memory uninstall` first writes every memory back into Claude Code's file memory: per
project `MEMORY.md` and one file per memory in `~/.claude/projects/<project>/memory/` (the format
`import` reads), and the global memories into `~/.claude/rules/graphersal-memory-global.md`, a user
rules file Claude Code loads in every project. A file that already says the same (whatever its
formatting) is left alone; a file nobody changed since its import gets the graph's newer version
(front-matter keys of other tools kept); a memory file you or the agent changed since is kept, and
the graph's version goes beside it as `<slug>.graphersal.md` (named in the report; never imported
back); a `MEMORY.md` that differs keeps its lines and gets the lines of the memories it lacks
appended; a superseded memory keeps its file (starting "SUPERSEDED by ...") but is not listed in
`MEMORY.md`; nothing is deleted. In a terminal it asks first
(default yes; `--no-export` skips it). `graphersal-memory export --to-files [--project ROOT]` does
the same at any time. Then uninstall removes exactly what `install` added: the MCP entries, the
hooks, the marked block, and the auto-memory switch goes back to what it was. A file nobody
changed since install gets its original bytes back (or is deleted again if install created it);
in a file Claude Code rewrote since, only our entries are removed. The acceptance test for this
is byte-for-byte: install, then uninstall, on a temporary home directory leaves the configuration
identical. Your memories stay in the data directory; `uninstall --purge` deletes them too (after
asking), and only what graphersal-memory created there: install refuses a data directory that
already holds other files. A configuration file that is a symbolic link (a dotfiles repository)
stays a link: install and uninstall write its target. A read-only file is refused before anything
changes, a write that fails midway puts back everything written so far, and only one install or
uninstall runs at a time. `status` shows what is installed (with the language, the model and the auto-memory
state), and `doctor` checks it (the model is downloaded and loads) and says how to fix what isn't
working.

No ports, no tokens, no system service. The harness starts `graphersal-memory mcp`, which starts a
small background daemon on demand. The daemon exits after two idle hours (configurable in
`config.json` in the data directory). See [How it works](docs/how-it-works.md).

## Does it help? Measure it

We don't ask you to take our word for it. `graphersal-memory bench` runs an A/B on **your own**
memory and sessions (Claude Code):

```sh
graphersal-memory bench --dry-run      # the plan, the sandboxes and the cost estimate; spends nothing
graphersal-memory bench                # shows the estimate, asks, runs, writes the report
graphersal-memory bench questions --review   # read and confirm the generated questions
graphersal-memory bench questions add "Which database do we use?" --answer "SQLite"   # add your own
graphersal-memory bench rule-changes   # does the next session follow a rule that changed?
```

- **Arms**: no memory; your file memory as it is (`CLAUDE.md`, `MEMORY.md` and the memory
  files); and graphersal-memory. An arm with your whole current setup (including another memory
  tool, if you use one) is planned.
- **Sandboxed**: every arm runs in a sandbox built for the run: its own MCP and settings files,
  an empty working directory, read-only tools, and for graphersal-memory a copy of your store.
  Your live configuration and store are never switched off or changed.
- **References from raw transcripts**: questions and reference answers are generated from your
  raw session transcripts, which every arm shares, never from our own summaries, which would
  favour us. Each names the turns it comes from, and you can review and edit them.
- **Your own questions**: add what you care about the assistant remembering (`bench questions
  add`, `import`, or a step when `bench` asks in a terminal), with your answer or with one looked
  up in your transcripts; the report scores them apart from the generated ones.
- **Judged and reported**: a judge scores each answer from 0 to 5 and flags confidently wrong
  answers. The report gives quality, cost, tokens, turns and cost per score point, per arm and
  per category.
- **Cost guard**: you see an estimate first; a run above `--budget` ($3 by default) is refused,
  and a run stops when its real spend reaches it. It runs through your own `claude -p`, on your
  account.

Method and a first run of the built-in bench: [docs/ab-testing.md](docs/ab-testing.md).

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
  transcripts for this reason; its first, much smaller run is in
  [docs/ab-testing.md](docs/ab-testing.md#a-first-run-of-the-built-in-bench).
- **The best-scoring configuration is reported.** Earlier configurations of the graph arm on the
  same questions scored 4.19 and 4.33.

That is why the A/B ships with the product: measure it on your own data before you rely on it.
Sources and the full method are in [docs/ab-testing.md](docs/ab-testing.md#our-first-measurement).

## Privacy

- **Everything stays local.** The memory store lives in your per-user data directory, and the
  daemon listens on a local socket that only your OS user can open. There are no network ports.
- **No telemetry.** Nothing is reported anywhere.
- **The A/B runs only when you run it.** `bench` sends excerpts of your raw transcripts (to
  generate questions) and your memory (to answer them) through your own harness, after showing
  the estimate and asking.
- **Summaries are opt-in.** Session summaries run only through your own harness (for example
  `claude -p` on your own subscription), never through an API key of ours or a service of ours.
  They run only after you agree, with a cost estimate first and a monthly budget cap. Above the
  cap, only raw turns are indexed, and summaries catch up later.
- **The network is used only when you ask.** That covers the embedding model download you choose
  in `install` (once, from Hugging Face; the built-in embedder needs none) and installing or
  updating the binary.

Details: [docs/privacy.md](docs/privacy.md).

## Supported harnesses

| Harness | Status |
|---|---|
| Claude Code | install, uninstall, import of the file memory, hooks, rule block (M3); past sessions and opt-in summaries (M4); the A/B, `bench` (M5) |
| opencode 1.x and 2.x | install, uninstall (Markdown export), rule file, session plugin, past sessions from its database, opt-in summaries and `bench` through `opencode run` (free models cost $0) (M7a: 1.x; M7b: 2.x, and the move of our entries after an upgrade from 1.x) |
| OpenAI Codex CLI | install, uninstall (Markdown export), rule block in `AGENTS.md`, SessionStart/SessionEnd hooks, past sessions from its rollout files, opt-in summaries through `codex exec`, `bench` with a local model (`--oss`), switching Codex's own memories off on request (M7c) |
| Cursor | install, uninstall (Markdown export), rule file `~/.cursor/rules/graphersal-memory.mdc`, sessionStart/sessionEnd hooks, past sessions from the transcripts the hooks name; no summaries or `bench` through Cursor (M7d; tested against fixtures, a live check is pending: [docs/cursor-live-check.md](docs/cursor-live-check.md)) |
| Desktop apps without a terminal (e.g. Claude Desktop) | later; the core is designed not to depend on a terminal or hooks |

Details: [docs/harnesses.md](docs/harnesses.md).

## Status

| Feature | Status |
|---|---|
| Command-line skeleton (`--help`, command set) | available (commands of later stages report "not implemented yet": the rule export of `export`) |
| Memory core: store, memory tools, project and global memories, `use`/`develop` profiles, MCP over stdio | available (M1) |
| Shared daemon + stdio shim, several sessions at once (`mcp`, `daemon`, the daemon part of `status`) | available (M2; Linux, macOS and Windows tested: the end-to-end daemon tests run in CI on all three, Windows over its `AF_UNIX` sockets) |
| One-command install / uninstall for Claude Code, `status`, `doctor`, import of the file memory (`import`), the managed rule block, the session hooks, `config.json` | available (M3; Linux, macOS and Windows tested, the real binary's install/hook/uninstall included; no harness has been run live on Windows yet; on Windows Claude Code's hooks run without a shell, so Git Bash is not needed: [Windows](docs/harnesses.md#windows)) |
| Hooks optional: the MCP server imports the project's memory files and queues the session indexing at every session start and end itself, for every harness (Claude Desktop and others without hooks included); `doctor` and `status` find Git Bash on Windows as Claude Code does | available (M11; tested on temporary homes with the real binary; the Windows command forms are tested with injected lookups on macOS, not yet on a Windows machine) |
| Interactive install wizard (Step N of M, defaults, re-run keeps your answers), memory language (English or any), embedding model download (potion-retrieval-32M / potion-multilingual-128M / built-in) with background re-embedding, switching Claude Code's auto-memory off, export back into the file memory on `uninstall` and with `export --to-files`, project identity across moved repositories (`projects list`, `projects merge`) | available (M6b; tested on temporary homes with a local fake Hugging Face server, no network in the tests) |
| Start context: every pinned, feedback and user memory put into the start of every session (Claude Code's SessionStart hook, else the MCP instructions; `pin`, `## Pinned` in an imported `MEMORY.md`, `start-context`, a budget in `config.json`), so Claude Code's auto-memory can be off without losing your rules | available (M14; tested on temporary homes against the documented hook and instruction contracts; not yet watched live in each harness: [Supported harnesses](docs/harnesses.md#the-start-context-what-reaches-the-model-at-a-sessions-start)) |
| Conflicts between memories: `remember` answers the nearest memories and a check for the agent's own model, `supersedes` keeps the history (superseded memories marked in every search, never in the start context), open conflicts and unchecked imported memories in `conflicts()`, settled with `settle`; the import, `status`, `doctor` and the start context name each conflict's file; `bench rule-changes` | available (M12; tested on temporary stores and homes; the answer format was measured before it was built: [How it works](docs/how-it-works.md#conflicts-between-memories)) |
| Import of `CLAUDE.md` rules | planned |
| Indexing past sessions (`import --sessions`, the hooks), opt-in summaries and facts through your own harness (`summaries estimate`, `status`, `enable`, `disable`), cost estimate and monthly budget cap | available (M4; tested with a fake harness, no model was called in the tests) |
| Built-in A/B (`bench`, `bench questions`): arms no memory / file memory / graphersal-memory, sandboxed, questions from raw transcripts, judge, report, cost guard, dry run | available (M5; Claude Code; tested with a fake harness, and one small real run). The arm with your whole current setup is planned |
| Release binaries (macOS arm64/x86_64, Linux x86_64/aarch64 static, Windows x86_64), `install.sh` / `install.ps1` with checksum verification, `SHA256SUMS`, build provenance | built (M6: the release workflow builds and smoke-tests every platform; the installers are tested against a local copy of a release); available once the first release is published |
| opencode: install/uninstall (`--harness opencode`, or both at once sharing one store), the session plugin, past sessions, summaries and `bench` through `opencode run` | available for 1.x (M7a) and 2.x (M7b): the line is detected with `opencode --version` (else from its database), an upgrade from 1.x is detected by `doctor` and moved by `install`, sessions are read from both lines' tables without duplicates. Tested on temporary homes with hand-built databases in each line's format and fake `opencode` binaries; real smoke runs with opencode 1.18.35 (M7a, and the M7b plugin) and 2.2.4 (M7b) on throwaway homes and a free model |
| Codex: install/uninstall (`--harness codex`), the hooks, past sessions, summaries through `codex exec`, `bench` with a local model | available (M7c), verified against Codex 0.167.0's source. Tested on temporary homes with hand-built rollout files and configurations and a fake `codex` binary; real smoke with Codex 0.167.0 and LM Studio (`qwen2.5-coder-14b-instruct`): registration, indexing and a summary through `codex exec --oss` worked; that model wrote its tool calls as text instead of calling them, so `remember`/`memories` through Codex are not yet shown with a local model |
| Cursor: install/uninstall (`--harness cursor`), the hooks, past sessions from the transcripts the hooks name | available (M7d), checked against Cursor's documentation of 2026-11-10 and tested on temporary homes with hand-built files in the documented formats; not yet run with a live Cursor (needs a login): [docs/cursor-live-check.md](docs/cursor-live-check.md). Summaries and `bench` do not run through Cursor's CLI (no cost reported) |
| Store maintenance: automatic checkpoints (`store.checkpoint_after`, default 16 MiB), pruning history older than `store.keep_history` (default 30 days), store sizes and WAL in `status`/`doctor` | available (M8; tested on temporary stores with an injected clock; start times measured in [How it works](docs/how-it-works.md#stores)) |
| Browser UI (`graphersal-memory ui`): overview, memories (search, create, edit, pin, forget), a graph of the memories and the decisions between them (sessions and facts on expand), what every session starts with (order, budget, literal preview), conflicts, past sessions; for every user, on loopback with a per-run token | available (stage UI; tested end to end with the real binary on temporary directories and once in headless Chrome on macOS; not yet run on Linux or Windows). It loads its libraries (Vue and others) from public CDNs, so it needs internet; no memory data leaves your machine ([privacy](docs/privacy.md)) |
| Desktop app | planned (M7) |

### Try it

Build the binary with Rust (`cargo install --locked --git
https://github.com/graphersal/graphersal-memory graphersal-memory`), then set it up for Claude
Code:

```sh
graphersal-memory install --dry-run   # the plan, nothing changes
graphersal-memory install             # shows the plan again and asks
graphersal-memory status              # what is installed, whether the daemon runs
graphersal-memory daemon restart      # after installing a new binary: open sessions reconnect
graphersal-memory daemon stop         # stop it; open sessions do not start it again
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
  saves them as global, and searches see the project's memories plus the global ones. Each
  project has a stable id: a repository found by its git remote is the same project wherever it
  is (moved, or a second clone: two clones of one remote share their memories, on purpose), and
  one found at a known path keeps its memories when it gets a remote. A moved repository whose
  remote changed too is reported by `doctor` and `install` when its first commit matches, and
  `graphersal-memory projects merge <from> <into>` moves its memories over (`projects list`
  shows the ids).
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
- The A/B: `graphersal-memory bench --dry-run` shows what a run would do and cost, `bench` runs
  it (see [Does it help? Measure it](#does-it-help-measure-it)). Reports go to `bench/` in the
  data directory.
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

**I already use another memory tool.** Keep it. Comparing it against graphersal-memory in the A/B
(an arm with your whole current setup, without changing it) is planned; today `bench` compares
no memory, your file memory and graphersal-memory. Import from another MCP memory tool goes through the agent itself:
both servers are connected, the agent reads from one and writes to the other.

## Built on Graphersal

graphersal-memory is built on [Graphersal](https://github.com/graphersal/graphersal), an embeddable
graph database and traversal engine in Rust with GQL and Gremlin, vector search, durable storage
and an MCP endpoint ([site](https://graphersal.github.io)). graphersal-memory is its first one-click
product and uses only its public API.

## Licence

Licensed under the [Apache License, Version 2.0](LICENSE). See [NOTICE](NOTICE).
