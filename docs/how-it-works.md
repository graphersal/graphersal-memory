# How it works

> How graphersal-memory 0.2.1 works inside: its processes, stores, past sessions, the start
> context, conflicts, the browser UI and the installation into a harness. Parts that are planned
> and not built yet (isolated project stores) say so; see also the [roadmap](roadmap.md).

## Processes

```text
harness ──stdio──> shim (graphersal-memory mcp --profile use) ──socket──┐
                                                                        v
browser ──http───> ui (graphersal-memory ui)                 ──socket──> daemon (graphersal-memory daemon)
                                                                          owns the stores, socket only;
                                                                          per store: one session thread
                                                                          + background job workers
```

- **Shim** (`graphersal-memory mcp`). Your harness starts it, one per agent session, exactly like
  any other stdio MCP server. It finds the project (for a developer, the repository root of the
  working directory), connects to the daemon's socket, starting the daemon if none is running,
  sends one handshake line, and then relays MCP messages in both directions, one line at a time.
  It holds no store; it remembers only what it needs to reconnect when the daemon restarts (see
  Crashes below).
- **Daemon** (`graphersal-memory daemon`). This is the only process that opens the stores. The
  first shim starts it, and it runs detached. It exits after an idle time: no connection, no
  background work and no `ui` command for two hours by default, which is configurable. It listens
  on the local socket only and opens no network port. If two shims start at the same moment, a
  start lock file decides which one spawns the daemon.
- **UI** (`graphersal-memory ui`, for every user). It serves the browser UI on loopback and
  talks to the daemon over the same socket; it runs until Ctrl+C, or until no browser tab has
  been open for 15 minutes. While it runs, the daemon stays; a forgotten browser tab keeps
  neither alive for long.

Several agent sessions, in several terminals or harnesses, can use the memory at the same time.
They all talk to one daemon, which is the single writer of each store. Calls of different sessions
run one after another on the store's session thread (memory-sized queries take milliseconds);
calls of one session may overlap and are answered in any order.

### The run directory

The daemon's files live in a per-user run directory: macOS
`~/Library/Application Support/graphersal-memory/run`, Linux `$XDG_RUNTIME_DIR/graphersal-memory`
(else `~/.local/state/graphersal-memory/run`), Windows `%LOCALAPPDATA%\graphersal-memory\run`.
`--run-dir DIR` or `GRAPHERSAL_MEMORY_RUN_DIR` moves it; a data directory chosen with `--data-dir`
or `GRAPHERSAL_MEMORY_DIR` gets `<data dir>/run`, so each data directory has a daemon of its own.
When the socket path in that directory would be longer than local sockets allow (104 bytes on
macOS: a long home directory), the run directory is `/tmp/graphersal-memory-<12 hex digits>` instead,
derived from the data directory, so every process of that data directory finds the same one;
only you can open it (`0700`). An explicit `--run-dir` that is too long is refused before
anything changes.

| File | What it is |
|---|---|
| `daemon.sock` | the socket |
| `daemon.pid` | locked by the running daemon for its whole life; the OS releases the lock when the process ends, also on a crash |
| `daemon.start.lock` | locked by the shim that starts a daemon |
| `daemon.log` | the daemon's log (`daemon.log.1`: the previous one, kept once it passes 1 MiB) |
| `daemon.stopped` | left by a daemon stopped on purpose (`uninstall`, `purge`, `daemon stop`; not `daemon restart`); open sessions and `ui` do not start a new daemon while it exists, and the next daemon removes it |

### Start, idle exit, crashes and upgrades

- **Start on demand.** A shim that cannot connect takes the start lock, tries again, and if nothing
  answers and no process holds `daemon.pid`, removes a leftover socket file and starts
  `graphersal-memory daemon --foreground` detached (its own process group on Unix, a detached
  process on Windows). A shim that started at the same moment waits for the lock and then finds
  the socket. A shim gives up after 30 seconds with an error naming the daemon's log.
- **Idle exit.** With no connection for the idle time, the daemon removes its socket, closes every
  store cleanly and exits. The idle time is, in this order: `daemon --foreground --idle 30m`, the
  environment variable `GRAPHERSAL_MEMORY_IDLE` (a daemon started by a shim inherits it), the
  settings file, two hours.
- **Crashes and restarts.** A socket file is treated as stale only when connecting fails and no
  process holds `daemon.pid`. If the daemon dies (killed, crashed, or replaced by an upgrade), open
  sessions keep their MCP server: each shim connects again, starting a new daemon when none
  answers (the store recovers at its open), replays the session's `initialize` and
  `notifications/initialized` itself (their answers stay with the shim) and goes on. A request
  that was waiting for the old daemon's answer gets a JSON-RPC error (code `-32000`: the server
  restarted, retry the call), so the agent never waits for an answer that is gone; requests sent
  during the reconnect wait and go to the new daemon. When the new daemon has another version,
  the shim tells the harness `notifications/tools/list_changed` so it lists the tools again. The
  session's start work (the memory-file import, the indexing request) is not repeated, and its end
  work goes to whichever daemon runs at the end. Only when no daemon answers within 30 seconds does
  the MCP server end with an error naming the log, as before. `ui` reconnects the same way. A
  daemon stopped on purpose (`uninstall`, `purge`) is not started again by open sessions, so a
  purge does not see its data recreated. `uninstall` stops the daemon when no harness has our
  parts any more (an uninstall of one harness of several leaves it running), and the daemon ends
  a summary's harness run it started before it exits.
- **No windows on Windows.** The daemon runs without a console, so every program it starts (the
  summaries' harness, `git`, `taskkill`) starts without a window of its own
  (`CREATE_NO_WINDOW`); up to 0.2.1 each summary run of `claude.cmd` opened a console window.
- **Upgrades.** When a shim meets an older daemon: with no other session open, the daemon exits
  and the shim starts the new binary; with sessions open, it serves the new shim too and exits
  when its last session closes, so nothing waits on an upgrade.
- `graphersal-memory daemon` (without `--foreground`) starts the daemon detached, or reports the
  one that runs; `graphersal-memory status` shows whether it runs (process id, version, uptime,
  connections, open stores).
- `graphersal-memory daemon stop` stops the running daemon and returns once it closed its stores
  and exited. Like `uninstall` and `purge`, it leaves `daemon.stopped`: open sessions do not start
  it again (their memory tools answer that it was stopped), but the next new session, `ui` or
  `graphersal-memory daemon` does. With no daemon it prints `daemon: not running` and exits 0.
- `graphersal-memory daemon restart` stops it the same way without the marker and starts a new
  one, for example after installing a new binary: open sessions and `ui` reconnect to the new
  daemon by themselves (an open session may start it first; either way one runs). With no daemon
  it just starts one.
- A connection that closes before sending its first line (a probe that only checks that the
  socket answers) is dropped without a log line; `doctor` asks the running daemon with a real
  request instead of such a probe.

## Socket, trust and the handshake

- On every platform, the shim and the daemon talk over a Unix domain socket (`AF_UNIX`, also
  available on Windows 10 1803 and later; the end-to-end daemon tests run over it in CI on
  Windows too). Windows named pipes stay the fallback if `AF_UNIX` proves unreliable there. On
  Windows the process that starts the daemon first makes its own standard handles
  non-inheritable, so the detached daemon never holds the harness's pipes open.
- The trust boundary is your OS user. On Unix the socket file is readable only by you (`0600`), in
  a directory only you can open (`0700`). On Windows the run directory is inside your own
  `%LOCALAPPDATA%`. Any process of your user can connect, which is no more than it could already
  do with the store files. There is no token to manage.
- The handshake is one JSON line from the shim:
  `{"shim_version", "protocol": 1, "profile": "use"|"develop", "project": {"root", "name"},
  "harness": {"name", "version"}, "data_dir"}`. The daemon answers `{"ok": true,
  "daemon_version", "store"}`, or `{"ok": false, "daemon_version", "error": {"code", "message"}}`
  with the codes `bad_handshake`, `protocol`, `unknown_profile`, `data_dir` (the shim works on
  another data directory), `store` (the store cannot be opened), and `restarting` or `stopping`
  (the shim tries again). After that, the connection carries plain MCP stdio framing in both
  directions, and the shim relays it line by line (keeping only the ids of unanswered requests and
  the session's opening, for a reconnect).
- A connection may instead start with a control line, `{"protocol": 1, "control": "status"}` (or
  `"stop"`), which gets one answer.

## Stores

- **Per-user data directory** by default (macOS `~/Library/Application Support/graphersal-memory`,
  Linux `$XDG_DATA_HOME/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`), not inside
  your repository. On Unix only you can open it (`0700`): `install` and the daemon create it
  that way and tighten an existing one at every daemon start (also a directory chosen with
  `--data-dir`); on Windows it is inside your own `%LOCALAPPDATA%`. A relative `--data-dir` or
  `--run-dir` is taken from the current directory and passed on as an absolute path.
- **One shared store by default.** Every memory carries its project, and search defaults to the
  current project plus your global memory: preferences and cross-project rules. The global
  memories live in the same store, without a project.
- **The settings file** (`config.json` in the data directory, written by `install` with the
  defaults and your answers): `{"version": 1, "daemon": {"idle": "2h"}, "memory": {"language":
  "english"}, "embedding": {"model": "minishlab/potion-retrieval-32M"}, "summaries": {"enabled":
  false, "harness_command": "claude -p --model sonnet", "monthly_budget_usd": 5}}` (plus the
  optional `summaries.since`, a date). `memory.language` (`english` or `any`) is the memory
  language rule of the `use` server's instructions and of the rule block; the daemon reads it for
  every new connection. `embedding.model` (a Hugging Face repository, or `hash-ngram/1` for the
  built-in embedder) is the model the daemon makes every store use; without it a store keeps the
  embedder it has. `store.checkpoint_after` and `store.keep_history` set the store maintenance
  (below). A missing file means the defaults; keys this version does not know are ignored, and
  `doctor` names them.
- **The embedding model.** Every memory, session, turn and fact has a vector (`emb`) the search
  compares, kept in step with its text by a vector rule of the schema. The built-in
  `hash-ngram/1` compares character n-grams: it finds shared words and spellings, not meaning.
  `install` offers a Model2Vec model instead (a static embedding model that runs on the CPU):
  it downloads the model once from Hugging Face into `models/<owner>/<name>/` in the data
  directory (checked against the SHA-256 Hugging Face states, with its model card, licence files
  and a manifest; a failed download leaves nothing behind), and names it in `config.json`. The
  daemon, when it opens a store (or when `install` tells a running daemon), imports the model
  into the store (it becomes the store's default model, and every copy of the store carries it
  with its licence) and switches the vector rules to it, which embeds every vertex again. That
  runs on a thread of its own, so no handshake waits; while the store re-embeds, memory calls
  wait for it (Model2Vec embeds tens of thousands of short texts per second).
- **Project identity** (`projects.json` in the data directory). Every project has a stable id,
  which memories, sessions, turns and facts carry: the project's root path when the memory first
  saw it, never changed afterwards (projects recorded before ids existed keep their path, so
  nothing had to be migrated). A session's directory is mapped to an id by up to three keys, in
  this order: (1) the git remote (`origin` first, then the others; normalized, so
  `git@github.com:me/app.git` and `https://github.com/me/app` are the same, credentials never
  stored): the same project wherever the clone is, so a moved repository keeps its memories and
  two clones of one remote share them, on purpose; (2) the path: a project seen there before,
  which records a remote the repository has now; (3) the root commit (`git rev-list
  --max-parents=0 HEAD`): never merged automatically, because forks and repositories made from a
  template share it, but `doctor`, `install` and `projects list` say "looks like project X,
  moved", and `graphersal-memory projects merge <from> <into>` moves the memories, sessions and
  facts of one project into the other (a memory of the same name keeps the more recently updated
  text) and the lookup keys with them.
- **The store registry** (`registry.json` in the data directory) says which store serves which
  project: `{"version": 1, "default": "default", "projects": {}}`. Today every project goes to
  the default store; the registry is where isolated projects will be listed.
- **Isolated projects** (planned): a project can get a store of its own, for example for a client
  under NDA, for handing a project to a team, or for an experiment. Your global memory is copied
  into it one way. Every MCP connection goes to exactly one store, and nothing references across
  stores.
- **Checkpoints.** A store is a snapshot plus the write-ahead log (WAL) of every commit after
  it; opening the store replays that WAL. The engine writes a new snapshot by itself once the WAL
  since the last one passes `store.checkpoint_after` in `config.json` (default `"16MiB"`; a size
  or a number of bytes; `0` = never automatically). The engine's own default, 256 MiB, is more
  than a memory store may ever write, so without this every start would replay the whole
  history. A checkpoint is merged from the last snapshot and the WAL after it, in the background
  (measured: 220 ms for the 5,000-turn store below, commits go on meanwhile). The embedding model
  is a blob the snapshots reference: it is stored once, never copied per snapshot.
- **History kept.** Old snapshots and WAL stay until they are pruned (the engine never deletes
  implicitly). The daemon prunes what is older than `store.keep_history` (default 30 days; a
  number of days, a duration such as `"12h"`, or `"forever"`/`0` for never): it checks each open
  store once a day and after each new snapshot, prunes up to the newest snapshot that is older
  than the kept window (so every moment of the window can still be recovered), and logs what it
  removed. The engine always keeps the last two snapshots and the WAL between them (the donors a
  repair rebuilds damaged data from) and verifies every kept snapshot before it deletes anything.
  A prune never stops a session. The bench copies the live store while the daemon serves it; the
  copy holds `stores/<name>.copy.lock`, and a prune that finds it held skips and tries again at
  the next check. `stores/<name>.maintenance.json` records the last check and prune.
- **`status` and `doctor`.** `status` shows each store's size on disk (snapshots, WAL, models),
  the WAL since the last snapshot (while the daemon runs: only the open store knows it), the last
  snapshot and prune, and the history kept. `doctor` reports a problem when the WAL since the
  last snapshot is more than four times `store.checkpoint_after` (checkpoints are not happening)
  or the last automatic checkpoint failed.
- **Start time.** Measured on 2026-11-10 (Apple Silicon laptop under other load, release build of
  this repository before 0.1.0, engine 0.6.0; median of 3 starts each; synthetic stores generated
  through the product's own paths in temporary directories: `import --sessions` of 100 Claude
  Code transcripts with 100 records each = 5,000 turns, plus 300 memories through `remember`). The
  time is from starting the shim (`graphersal-memory mcp`, which starts the daemon) to the answer;
  "open" is the engine's own figure for opening the store, which includes loading the embedding
  model the schema names; "replayed" is the WAL commits replayed at the open.

  | Store | Model | open (replayed) | `initialize` | first `memories` | first `recall` | next `recall` |
  |---|---|---|---|---|---|---|
  | empty | built-in `hash-ngram/1` | 19 ms (14) | 0.08 s | 0.08 s | 0.08 s | 1 ms |
  | empty | potion-retrieval-32M (125 MiB) | 448 ms (17) | 0.50 s | 0.50 s | 0.50 s | 1 ms |
  | empty | potion-multilingual-128M (506 MiB) | 1.71 s (17) | 1.76 s | 1.76 s | 1.76 s | 1 ms |
  | 5,000 turns + 300 memories, no checkpoint (WAL 16 MiB) | built-in | 63 ms (614) | 0.12 s | 0.12 s | 0.21 s | 85 ms |
  | same, no checkpoint (WAL 29 MiB) | retrieval-32M | 500 ms (618) | 0.55 s | 0.56 s | 0.56 s | 1 ms |
  | same, no checkpoint (WAL 24 MiB) | multilingual-128M | 1.82 s (618) | 1.87 s | 1.87 s | 1.87 s | 1 ms |
  | same, after the checkpoint | retrieval-32M | 442 ms (0) | 0.46 s | 0.46 s | 0.46 s | 1 ms |
  | same, after the checkpoint | multilingual-128M | 1.72 s (0) | 1.74 s | 1.74 s | 1.74 s | 1 ms |
  | same, the daemon already running | retrieval-32M | (open) | 0.04 s | 0.04 s | 0.04 s | |
  | same, the daemon already running | multilingual-128M | (open) | 0.04 s | 0.04 s | 0.04 s | |

  What this says: the model load is most of a cold start (about 0.43 s for retrieval-32M and
  1.7 s for multilingual-128M); replaying 600 commits and 16-29 MiB of WAL costs 40-100 ms, so
  the 16 MiB checkpoint threshold bounds the replay at about that; the first query embedding
  costs nothing noticeable once the model is loaded. A cold start happens once per daemon life
  (it stays up for `daemon.idle`, two hours by default). Caveats: one machine, a busy one; the
  synthetic turns are random words of realistic length, not real conversations.
- **When the model loads.** The engine loads the model a store's schema names while it opens the
  store (so a store whose model cannot load is refused at the open, not at the first search),
  and every MCP call, also one that needs no vectors, needs the open store. So the daemon opens
  the default store as soon as it starts, beside the first handshake: a session whose daemon a
  hook (or `graphersal-memory daemon`) started a moment earlier finds the store open, and a call
  that arrives while it opens waits for it and then answers from the complete store (never a
  partial answer). Answering calls that need no vectors before the model is loaded would need an
  engine option to load schema models at the first embedding instead of at the open, which
  the engine (0.6.x) does not have.
- Background work (backups, checkpoints, session indexing, summaries, imports) runs off the thread
  that answers the agent, so a long job never makes a memory lookup wait. The daemon ticks each
  store's scheduled jobs and runs them on worker threads of their own; the session indexing runs
  on a thread of its own and writes in small units (see below).

## Past sessions

- **Which sessions.** Every Claude Code transcript `~/.claude/projects/<project>/<session>.jsonl`
  that is finished: unchanged for 30 minutes, or named by the `SessionEnd` hook. The hooks and the
  MCP server's start and end ([without hooks](#without-hooks)) ask for the indexing. The
  transcript a running session writes is left for later, and the transcripts of sub-agents (in
  `<session>/subagents/`, and sub-agent records inside a transcript) are never indexed.
- **What is kept.** A `session` vertex (`session:<transcript name>`: the title Claude Code
  generated, start and end, branches, the files edited relative to the project, the number of
  turns, Claude Code's own compaction summaries) and one `turn` per prompt
  (`turn:<session>:<n>`: the prompt with the messages sent while the assistant worked and the
  answers to its questions, the assistant's text replies until the next prompt, the names of the
  tools it used). Tool inputs and results are not kept, except the names of the memories the
  session wrote through the memory tools: each call of `remember`, `pin`, `settle` or `forget`
  (in every harness's transcript: Claude Code, opencode 1.x and 2.x, Codex, Cursor, CodeWhale,
  DeepSeek Harness) links the
  session to that memory with a `wrote` edge (the calls in order, the first and last time, how
  many), when the memory still exists. A session indexed before this was added is linked once,
  at the next indexing run, from its transcript if that still exists. Memories written in the
  browser UI and imported from files have no session. Every vertex carries the session's
  project: its working directory's git root, as for an agent session started there; a session's
  vertices are visible only from that project.
- **Robust parsing.** A line that is not JSON, a record type this version does not know and a
  last line without its line end (a session being written) are skipped.
- **Idempotent.** The session vertex carries a fingerprint of the transcript lines its turns came
  from, set by the last batch of a session's writes; `sessions/claude-code.json` in the data
  directory remembers each transcript's size, time and fingerprint, so an unchanged transcript is
  not read again and a grown one (a resumed session) is indexed again.
- **Never in the way.** The hooks only send one control line to the daemon (`index_sessions`),
  answered at once, and return. The daemon's indexer thread parses off the store's session thread
  and writes each session as one call for its properties plus calls of at most 40 turns, each a
  short job on the session thread, so an agent's memory call waits at most for one such unit. The
  daemon does not exit idle while the indexer works. `graphersal-memory import --sessions` sends
  the same request and shows the progress (`sessions_status`).
- **Summaries (opt-in).** With `summaries.enabled`, after the raw turns the indexer summarizes
  each session without a summary of its current version through your harness (one call per
  session, within the monthly budget; see [Privacy](privacy.md#what-is-sent-to-your-harness-and-when)),
  stores the English summary on the session (`name` = headline, `summary`, `body`) and splits it
  into `fact` vertices (`fact:<session>:<kind>:<i>`, kinds `decision` with its why, `correction`,
  `outcome`, `open_thread`), linked from the session by `contains`. `sessions(text)` searches
  these facts and answers each with its kind, why, date, session id and headline; `session(id)`
  gives the whole summary. A failed summary is tried twice per session version, then waits until
  the session changes.

## The start context

The memories that always apply reach the model at the start of every session, without the agent
having to search for them: every **pinned** memory (any kind), and every **feedback** and **user**
memory, of the session's project and the global ones. This replaces what Claude Code's
`MEMORY.md` does for its own memory.

- **Order**: pinned first, then feedback, then user; within each by `order` (a priority the user
  can give, lower first), then newest first. `memories()` without text lists them in that order,
  then the other memories.
- **Text**: grouped like `MEMORY.md` ("How to work" = feedback, "About you" = user, "Pinned" =
  the other pinned ones), one line per memory: `- <title> — <hook or summary> (memory: <slug>)`,
  then a line naming `memory(slug)` for the full text and `memories(text)` for the rest.
- **Budget**: `memory.start_context_chars` in `config.json` (default 8,000 characters; `0` turns
  it off). Over it, memories are kept in the order above and the text ends with "N more not
  shown: memories(kind: ...)".
- **Pinning**: the agent pins on request with `pin(slug, pinned, order?)` (or `remember(...,
  pinned)`); memories under a `## Pinned` heading of Claude Code's `MEMORY.md` are imported as
  pinned, and an index line moved out of that heading unpins the memory at its next import.
  The import sets `order` from the index line's position, so the start context and `memories()`
  keep the `MEMORY.md` order; an order set later (`pin`) stays until the line moves. The export
  back (`uninstall`, `export --to-files`) writes the pinned memories under `## Pinned` again,
  with their titles and hooks, so a pin survives files -> graph -> files.
- **Delivery**: the session-start hooks (Claude Code: `additionalContext`; Cursor:
  `additional_context`) and the `use` server's instructions of each connection, computed when the
  connection opens (opencode, Codex, CodeWhale and DeepSeek Harness get it only there). A Claude
  Code connection whose settings hold our SessionStart hook leaves it out of the instructions, so
  it is not sent twice; Claude Code without the hook, CodeWhale and DeepSeek Harness get the
  short form, cut to 4,096 characters of instructions (CodeWhale cuts server instructions there,
  dsh rejects a server whose instructions are too long; our server entries name the harness in
  their `env`, so the server knows); per harness: [Supported
  harnesses](harnesses.md#the-start-context-what-reaches-the-model-at-a-sessions-start).
- **Superseded and conflicting memories**: a superseded memory never starts a session; open
  conflicts follow the groups with both versions ([Conflicts](#conflicts-between-memories)).
- **Seeing it**: `graphersal-memory start-context [--project DIR]` prints the text; `status` and
  `doctor` show "start context: N memories, X of Y characters (Z not shown)" for the current
  directory's project (when the daemon runs), then the open memory conflicts with their files and
  the imported memories not yet checked.

## Conflicts between memories

graphersal-memory merges memories from several places (what an agent remembers, the imported
memory files, project and global scope) and keeps every decision over time, so rules can
contradict each other and an old decision can be replaced by a new one. The memory finds the
candidates; the agent's own model decides (no model call of graphersal-memory's, no cost).

- **Check on write.** `remember` answers, besides what it stored, the five nearest existing
  memories (`nearest`: slug, scope, kind, summary, source = `remember` or the file it was
  imported from, `updated_at`), found with the same hybrid scoring as `recall` on the new
  memory's slug and summary, over the project's and the global memories, without the memory
  itself and without memories already superseded, and a `check` text: for each nearest memory
  the agent picks **compatible** (the default), **replaces** (the same rule in other words: keep
  one), **supersedes** (a later decision: `remember` again with `supersedes: [old slug]`) or
  **contradicts** (record it with `settle(new, "conflict", old)` and ask the user). Both are
  left out when nothing is near (about 150 tokens otherwise).
- **Scope.** A project memory that narrows a global one is compatible, not a conflict (a
  conflict exists only within one scope): `settle(project slug, "narrows", global slug)` records
  the note "narrows the global rule <slug>"; `settle` refuses a conflict across scopes.
- **History, not deletion.** `supersedes` (newer -> older, with the time it was recorded) keeps
  both memories. Every search (`memories`, `recall`) and `memory(slug)` mark the old one: its
  summary starts with "SUPERSEDED by <slug> on <date>:", and `memory(slug)` shows the whole
  `chain` from the oldest to the newest decision. A superseded memory is never hidden, because
  it leads the agent to its successor; it never appears in the [start context](#the-start-context).
  Superseding a memory ends its open conflicts.
- **Open conflicts** are `conflicts_with` edges until the user decides. `conflicts()` lists them
  with both versions and their files, the imported memories not yet checked (with their nearest
  memories and the check), the narrowing notes and the files that still hold a superseded rule;
  `settle(slug, decision, other)` records a decision (`compatible`, `narrows`, `supersedes`,
  `conflict`; `settle(slug, "compatible")` alone ends the check of an imported memory). A search
  hit with an open conflict names the other memory; `memory(slug)` lists both sides.
- **Imported memories.** The import passes each file to `remember`: the memory's `source` is the
  file, `imported_at` the time of the import, and `updated_at` the file's last change (when the
  rule was decided, not when it was imported). An imported memory with similar memories of
  another source (another memory directory, another harness, or `remember`) is marked
  `unchecked` until an agent settles it; the files of one memory directory already stood side by
  side in your setup, so they are not checked against each other. Memories imported before this
  marking existed get their file and import time once, from the import's records, when the
  daemon next starts. The import's report, `status`, `doctor` and the
  start context name the file of every open conflict, and every answer that supersedes, settles
  or forgets an imported memory tells the agent to have the user edit or delete that file: a
  file the harness loads into the context still holds the old rule. An unchanged file never
  brings a superseded or forgotten memory back: the import state records the file text's own
  fingerprint and that its memory was forgotten, so a moved `MEMORY.md` line alone imports
  nothing, and an import never removes a `supersedes` edge.
- **Edits win over the file.** An imported memory can be edited, by your agent (`remember`) or
  in the browser UI. The edit detaches it from its file: it loses `imported_at` (and
  `unchecked`) and keeps the file it came from as `edited_from`, so the provenance stays
  visible; from then on it is a normal memory. When that file changes later, the import keeps
  the edited text and writes nothing (`remember` with the file answers `kept: "edited"`): it
  records the file's new fingerprint, so it does not try again every session, and its report
  says "<file> changed, but its memory <slug> was edited in graphersal-memory; the edited text
  was kept". The same holds for the memories imported from another memory tool.
- **At session start** the start context lists the open conflicts (both versions, which of them
  is always loaded, their files; at most five, then "N more") and the number of imported
  memories waiting for their check, both naming `conflicts()`; Claude Code's start line adds the
  same counts.
- **Measured** (an experiment on 2026-11-12, before this was built): on 64 hand-labelled pairs
  (contradictions, superseded decisions, near duplicates, similar but compatible, unrelated)
  among 232 unrelated memories, the nearest five held the conflicting or superseded memory in
  94 % of the cases (one project and the global memories; 91 % over everything), and Claude
  Haiku with this answer format found 100 % of them with no compatible pair flagged (22 pairs);
  in 18 follow-up sessions after a rule change, 18 followed the newer rule with the
  SUPERSEDED mark, 17 without it, 16 with the old memory hidden and 13 with the old rule still
  in a loaded file. Caveats: one labeller, a synthetic corpus apart from one real case, single
  turns rather than a live tool loop, small samples. `graphersal-memory bench rule-changes`
  repeats the last measurement (arms plain / marked / file) through your `claude -p`.

### Memories against instruction files

The harness loads its own instruction files into every session (Claude Code: `~/.claude/CLAUDE.md`,
`~/.claude/rules/*.md`, the project's `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md` and
`.claude/rules/*.md`; opencode: its `AGENTS.md` and `instructions` entries, the project's
`AGENTS.md`, Claude Code's files where it reads them; Codex: `~/.codex/AGENTS(.override).md`,
the project's `AGENTS(.override).md` and its fallback names; Cursor: `~/.cursor/rules/*.mdc`,
the project's `.cursor/rules/*.mdc`, `.cursorrules` and `AGENTS.md`; CodeWhale: its one global
file (the first of `~/.codewhale/AGENTS.md`, `~/.agents/AGENTS.md`, `~/.deepseek/AGENTS.md`, then
the same names `instructions.md`) and the project's `AGENTS.md`; DeepSeek Harness:
`$DSH_HOME/AGENTS.md` (`~/.dsh/AGENTS.md`), `$DSH_AGENTS_HOME/AGENTS.md` (`~/.agents/AGENTS.md`)
and the project's `AGENTS.md`, `CLAUDE.md`, `AGENTS.local.md` and `CLAUDE.local.md`). A memory can contradict
one of their lines, for example a memory "use pnpm" against a `CLAUDE.md` line "always use npm".
graphersal-memory never imports those rules (the agent would get them twice) and never edits
those files; it finds the pairs worth a look, and the agent or you decide.

- **Which files**: those of the harnesses graphersal-memory is installed in (else the harness
  that started it), the user-level ones and the session project's; never graphersal-memory's own
  rules files, the exported global memories, or its managed block inside a shared file.
- **A rule** is one list item, paragraph or block quote (a wrapped line belongs to its item)
  that is a directive: it has an English directive word (never, always, must, should, don't /
  do not, prefer, avoid, instead, required, forbidden, prohibited, make sure, only when / if /
  after / through / via, or "No ..." opening a sentence), or it stands under a heading that
  names rules (Rules, Conventions, Guidelines, Do, Don't, Main rule, Policies, ...), or it is in
  a rules file (a `rules` directory, a `.mdc` file, `.cursorrules`, a file with a front matter).
  Descriptions of the project ("`io`: GraphSON 3.0 ...", "the core library") are no rules: a
  memory on the same topic never contradicts them. The words are English: a line in another
  language counts only under a rules heading or in a rules file. Headings, tables, code blocks,
  HTML comments, a front matter, link lists and units under 20 characters or 4 words are left
  out, a long paragraph is cut at 400 characters. It is kept as an
  `instruction` vertex with its file, line, text and harnesses, global for a user-level file,
  else of the project, known by a fingerprint of the project, the file (a project's file by its
  path inside the project, so two checkouts share it) and the text: a moved line keeps its
  decisions, a changed or deleted line is dropped with them. These vertices are never found by
  `recall` or `memories`.
- **When**: at every session start (the MCP server's start, and Claude Code's and Cursor's
  session-start hooks before the start context) and with `graphersal-memory import` (always);
  nothing is sent when the files are unchanged since the last finished sync
  (`<data dir>/import/instructions.json` records their fingerprint).
- **Candidates**: the daemon looks up the nearest memories of each new rule with the `recall`
  scoring (a project's rule against the project's and the global memories, a user-level rule
  against the global ones) in the background, after it answered the sync, so a session start
  never waits for it (batches of 10 rules, so the agent's calls wait for one batch at most;
  the start context shows the candidates found so far), and
  `remember` does the same the other way for a memory whose text changed (a project memory
  against its project's lines, a global one against every line it sees). A pair is a candidate
  when the memory is among the rule's three nearest, has a semantic similarity of at least 0.6
  AND shares at least 2 content words with the rule (words of 3 letters or more, stop words and
  directive words left out, the first 6 letters compared), and is not superseded; no new
  candidates while 20 wait for their check. The bar was raised on 2026-11-14 after all 50
  candidates found on real instruction files were merely on the same topic; the candidates
  then waiting were dropped once and every line scanned again (settled decisions stay).
  Candidates are the agent's work: you see only the pairs it confirmed as a conflict.
- **Only with an embedding model**: with the built-in `hash-ngram/1` embedder there is no
  check (a real contradiction scores far under the bar there, and its candidates were only
  lines on the same topic): no candidates are searched, the start context says nothing,
  `status`, `doctor`, the overview and the Conflicts tab say "off (needs an embedding model)",
  and `install` says so with the built-in embedder. The lines are still synced; once a model
  is installed, the daemon scans the lines waiting by itself (the schema's
  `meta.instruction_check`, set from the embedder).
- **Deciding**: `conflicts()` lists the candidates (`rules_unchecked`, both texts, the file and
  line) and the open ones (`rules_open`); `remember` names the lines near a new memory
  (`instruction_lines`). `settle(slug, "compatible", rule: <id>)` dismisses a pair until the
  memory's text or the line changes; `settle(slug, "conflict", rule: <id>)` records an open
  conflict, which the start context (at most five, then "N more"; one line asks the agent to
  check the waiting pairs), Claude Code's start line, `status`, `doctor` and the UI show until
  it is
  resolved: the memory is outdated (forget it, or remember the new rule with `supersedes`; an
  imported memory's file is edited instead), or the file is outdated (you edit that line; the
  conflict closes by itself at the next sync, when the line changes or goes).

## The browser UI

`graphersal-memory ui` opens your memory in the browser: what every session starts with, the
memories, the conflicts and the past sessions. It is for every user, and it changes nothing
unless you click: every change goes through the same memory functions your agent uses
(`remember`, `forget`, `pin`, `settle`), recorded as written by the UI (`source` "ui"), so the
conflict checks, the history and the start context work exactly as for the agent.

- **Overview**: how many memories (by kind, project and global), what is always loaded and how
  many characters it takes, open conflicts and imported memories still to check, indexed
  sessions, and the lines `status` and `doctor` print.
- **Memory**: the list (filters: kind, project or global, written by the agent / here /
  imported file / other memory tool, replaced ones), search by meaning (the same search your
  agent gets), a memory in full, and create, edit, pin and forget (forget always asks). A memory
  imported from a file can be edited here too: the edit is kept, a later change of that file no
  longer replaces it ([edits win over the file](#conflicts-between-memories)), and the memory
  shows "Edited here; first imported from <file>". It is not forgotten here: edit or delete its
  file instead. Before an edit is saved, the memory is read again: when an agent changed it
  meanwhile, nothing is saved.
- **Graph**: the memories and the decisions between them as a graph: replaces (newer to older,
  an arrow), open conflicts (red), narrows (a project memory to the global one it makes an
  exception to) and related memories (thin grey); colour by kind, a thick border for the
  memories every session starts with, a dashed one for imported memories, faded when replaced,
  with a legend. Sessions and facts appear only when you expand a memory (double click, or
  Expand on its card): the sessions that wrote it (an arrow from the session, its card naming the
  tool calls), what it is about and what is about it, and the facts of those sessions. A session
  is linked once it has ended and was indexed, so a memory written in the running session shows
  none yet; a memory imported from a file says so instead (no session wrote it), and one written
  in the UI has no session either. The graph only reads; its cards
  write as the other tabs do (open in Memory, pin, settle a conflict, forget with a question,
  never an imported memory).
- **Always loaded**: exactly what a session of a project starts with, in delivery order and
  grouped as the agent sees it, the budget bar, the literal text, labels for what does not fully
  reach the agent ("cut: no room" over the budget, "shortened" when a line's text is cut at 300
  characters; the tooltip says what the agent gets instead), pin/unpin in "Pinned", move up/down,
  edit a memory's start line, and the budget (`memory.start_context_chars`). "How to work" and
  "About you" have no pin checkbox: those memories are always loaded, and a note says that the
  order only decides what is cut first when space runs out (from the end: About you, then How to
  work, then Pinned); one that is pinned anyway shows "kept first" with a button that removes the
  mark. Warnings: a budget too small for the pinned memories, a pinned
  memory in an open conflict or replaced.
- **Conflicts**: open conflicts side by side (which one replaces the other, both hold, forget
  one, later), and imported memories still to check, one by one; then "Memory vs instruction
  files": a memory beside the line of an instruction file your agent found it to contradict
  (file and line; the pairs still waiting for the agent's check are one quiet line, "N pairs
  waiting for the agent's check", with a collapsed list), with "Both hold", "Memory is outdated" (forget it; not for an imported memory, whose file is
  the one to edit) and "File is outdated: edit <file> line N" (recorded as a conflict; nothing
  here writes the file, and the pair closes by itself when that line changes).
- **Sessions** (read-only): the newest sessions by day, search of their decisions and other
  facts by meaning, a session in full with its conversation on demand.

The project switcher in the page's top bar, beside the theme switch, holds for every tab: one
project (its memories and the global ones), the global memories alone, or all projects. The top
bar also shows the daemon's state and, when the setup has problems, a warning that opens the
Overview.

The header shows graphersal-memory's own version; the engine's version is in its tooltip and on
the Overview.

How it is guarded: `ui` listens on `127.0.0.1` only (a random port, `--port` to choose); every
API request needs a token of 256 random bits that only the opened tab has (it reaches the
browser through a launch file in the run directory, never through a command line, and lives in
the tab's session storage, from where the page hands it to the engine as the request header
`Authorization: Bearer <token>` through the playground's host hook `window.GRAPHERSAL_ENGINE`;
it is never put into an address); requests from another site are refused (Host and Origin checks, no
cross-origin headers); `GET` never changes anything. The daemon stays socket-only: `ui` hands
each request over the socket to the daemon, which answers only the memory functions and the
UI's own read and reorder functions (`overview`, `memory_list`, `set_order`, `session_list`,
`memory_graph`, `memory_neighbours`),
never the internal ones (indexing, merge, export, migrations), under the same access policy as
the agent's `use` server.

The page is Graphersal's playground with the `memory` profile, and it loads its libraries (Vue
and others) from public CDNs, each pinned with a checksum (SRI): the UI needs internet, and the
CDN sees that the page was opened; no memory data is sent there.

## Profiles: the two MCP servers

| Server | Default | Tools | Enforcement |
|---|---|---|---|
| `use` | on | search and read memories, remember, forget, pin, list and settle conflicts, search and read past sessions | the engine's access policy allows only what these tools write |
| `develop` | off | the above plus schema, saved queries, indexes, health, benchmark | for improving the memory's structure; enable it when you need it, disable it so it costs no context |

The daemon chooses the profile per connection, from the handshake. A client cannot raise its own
profile from inside MCP.

How `use` is enforced today: the server offers only the memory functions as tools, and any other
tool call is refused. The queries run under an engine access policy built at start from what the
memory functions are inferred to write: reading, plus creating, updating and deleting data. Schema
and saved-query changes are denied. The policy works on kinds of writes, not on labels, so the
tool list is what keeps a `use` client to the memory functions.

The memory functions are saved queries kept in the store (folder `memory`, marked as the
product's). When graphersal-memory is upgraded, it replaces them and the schema with its own
version, so edit copies under another name. Every memory function (also `session`, which reads
one past session by id) takes the project as a parameter. The server sets it on every call to the
connection's project and hides it from the tool list, so an agent cannot read another project's
memories or sessions by passing a different one, by id or by an id prefix.

## The engine

The stores, queries, embeddings, access policies and the MCP endpoint come from
[Graphersal](https://github.com/graphersal/graphersal): `graphersal` (the engine), `graphersal-mcp`
(the MCP endpoint, connections, profiles and stdio framing) and `graphersal-serve` (the hosted
graph runtime). graphersal-memory adds the product around them: daemon, shim, store registry,
installation into harnesses, the memory schema and functions, import and export, and the A/B.

## Installation into a harness

`graphersal-memory install` writes only managed parts into the harness's configuration and records
what it changed in the data directory (`install/claude-code.json`, with a backup of every file
before its first change in `install/backups/`); `uninstall` reverts exactly that. The details per
harness are in [Supported harnesses](harnesses.md).

The import of the file memory goes through the memory functions like any agent call: the product
opens a `use` connection to the daemon for each project and calls `remember`, so the store keeps a
single writer. `import/claude-code.json` in the data directory remembers the fingerprint of every
imported file, so only new and changed files are sent again.

The export back (`uninstall`, `export --to-files`) asks the daemon for every memory and writes
them in the format the import reads: per project `MEMORY.md` and `<slug>.md` with the front matter
`name`, `description`, `metadata.type` in the project's memory directory, the global memories into
one file the harness loads in every project (Claude Code: `~/.claude/rules/graphersal-memory-global.md`).
A file that already holds the same text, or says the same in other formatting (CRLF, blank
lines, a folded description, metadata keys of others), is left alone; a file whose text is still
what the import read (its fingerprint in `import/claude-code.json`) gets the graph's version, with
the front-matter keys the export does not write kept; a file edited since is kept and the graph's
version written beside it as `<name>.graphersal.md`, which the import never reads; nothing is
deleted. A superseded memory is written with a first line "SUPERSEDED by <slug> on <date>" and
left out of `MEMORY.md`; a memory in an open conflict starts with "CONFLICT with <slug>"; the
import takes both lines off again. Importing an
export gives the same memories back (a test checks it); links the body does not show get a
`Related: [[slug]]` line.

### Other memory, and facts that come back

Install, `status` and `doctor` read the other memories on the machine (built-in memories, other
memory tools' MCP entries, memory plugins, instruction files, external writers; see
[Supported harnesses](harnesses.md#other-memory-beside-graphersal-memory)) from files only. What
the user chose is kept in `install/other-memory.json` (the tools the rule block names read-only,
the ones kept as they are); an MCP entry install switched off is recorded in the harness's install
state beside the file it was in (`other_memory_off`: the removed entry, or the switch and its value
before), so `uninstall` can switch it on again. An import from another tool goes through a `use`
connection without a project (an empty `project.root` in the handshake: global memories only) and
records each item's fingerprint in `import/other-memory.json`.

Every imported memory knows its file (`source`, `imported_at`; once edited here, `edited_from`;
[conflicts](#conflicts-between-memories)). A changed file whose memory was edited in
graphersal-memory since its import keeps the edit (the report says so). When the import of a
changed file finds that the memory was forgotten in the graph (or changed there by an agent
before graphersal-memory recorded edits as `edited_from`), it still writes the file's version
(the file changed: that is a decision too) and records `returned` in the file's record of
`import/claude-code.json`; the import's report, `status` and `doctor` name the file and the ways
out. `import --ignore PATH` adds a file or directory to the record's `ignored` list: the import
skips it from then on (`--unignore` takes it off again).

### Without hooks

The hooks are optional. Every harness starts the MCP server (`graphersal-memory mcp`) when a
session starts and closes it when the session ends, so the server does the hooks' work itself:

- **At the start**, right after it connected to the daemon and beside the MCP traffic (the
  agent never waits for it): it imports the new and changed Claude Code memory files of the
  session's project, and queues the indexing of finished sessions for every harness
  graphersal-memory is installed in (else the harness that started it, when it can tell: Claude
  Code sets `CLAUDECODE`).
- **At the end** (the harness closes its input): it queues the indexing again, when the daemon
  still runs.
- **The start line and the start context** the session-start hook delivers are covered by the
  server's instructions, which every MCP client reads at the start: they name the memory tools,
  say to write memories there, not into memory files, and end with the start context (below).

When the hooks run too, nothing happens twice: the import holds the import lock and skips files
whose fingerprint it already has, and the daemon merges index requests and skips unchanged
sessions. Inside a summary run (`GRAPHERSAL_MEMORY_HOOK=off`) the server does nothing of this.

What still needs a hook: the server does not know which transcript is its own session's, so the
session that just ended is indexed once it has been unchanged for 30 minutes (at a later session
start or end, or `import --sessions`) instead of right away; the imported-files count in the
start line; and Cursor's transcripts, which are known only from what its hooks report.

The code talks to a harness through one interface (the `Harness` trait in
`src/harnesses/agent.rs`: its program, files, install and uninstall parts, status and doctor,
and optional capabilities for its own memory, other memory, sessions, hooks and headless runs),
and one registry names every harness (`src/harnesses/mod.rs`), so the next harness is a new
module that implements it rather than a new code path.
