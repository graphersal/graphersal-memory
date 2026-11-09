# How it works

> Pre-release: this page describes the target design. Implemented so far (M1 to M4): the store in
> the per-user data directory, project and global memories, the two profiles with their access
> policy, the daemon with the shim, the socket, the handshake, start on demand, idle exit, upgrade
> and crash recovery, the settings file, the project records, the installation into Claude Code
> with its hooks, the import of the file memory, and the background indexing of past sessions
> with opt-in summaries. Not yet: the `ui` command, isolated project stores; see the
> [status table](../README.md#status).

## Processes

```text
harness ──stdio──> shim (graphersal-memory mcp --profile use) ──socket──┐
                                                                        v
browser ──http───> ui (graphersal-memory ui; develop)        ──socket──> daemon (graphersal-memory daemon)
                                                                          owns the stores, socket only;
                                                                          per store: one session thread
                                                                          + background job workers
```

- **Shim** (`graphersal-memory mcp`). Your harness starts it, one per agent session, exactly like
  any other stdio MCP server. It finds the project (for a developer, the repository root of the
  working directory), connects to the daemon's socket, starting the daemon if none is running,
  sends one handshake line, and then relays MCP messages in both directions. It holds no state.
- **Daemon** (`graphersal-memory daemon`). This is the only process that opens the stores. The
  first shim starts it, and it runs detached. It exits after an idle time: no connection, no
  background work and no `ui` command for two hours by default, which is configurable. It listens
  on the local socket only and opens no network port. If two shims start at the same moment, a
  start lock file decides which one spawns the daemon.
- **UI** (`graphersal-memory ui`, develop mode). It runs while its terminal runs, serves the
  browser UI on loopback and talks to the daemon over the same socket. A forgotten browser tab
  does not keep the daemon alive.

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

| File | What it is |
|---|---|
| `daemon.sock` | the socket |
| `daemon.pid` | locked by the running daemon for its whole life; the OS releases the lock when the process ends, also on a crash |
| `daemon.start.lock` | locked by the shim that starts a daemon |
| `daemon.log` | the daemon's log (`daemon.log.1`: the previous one, kept once it passes 1 MiB) |

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
- **Crashes.** A socket file is treated as stale only when connecting fails and no process holds
  `daemon.pid`. If the daemon dies, open sessions see their MCP server end with an error naming the
  log (restarting the MCP server in the harness reconnects); the next session starts a new daemon,
  and the store recovers at its next open.
- **Upgrades.** When a shim meets an older daemon: with no other session open, the daemon exits
  and the shim starts the new binary; with sessions open, it serves the new shim too and exits
  when its last session closes, so nothing waits on an upgrade.
- `graphersal-memory daemon` (without `--foreground`) starts the daemon detached, or reports the
  one that runs; `graphersal-memory status` shows whether it runs (process id, version, uptime,
  connections, open stores).

## Socket, trust and the handshake

- On every platform, the shim and the daemon talk over a Unix domain socket (`AF_UNIX`, also
  available on Windows 10 1803 and later). Windows named pipes are the fallback if `AF_UNIX`
  proves unreliable there.
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
  directions, and the shim only relays bytes.
- A connection may instead start with a control line, `{"protocol": 1, "control": "status"}` (or
  `"stop"`), which gets one answer.

## Stores

- **Per-user data directory** by default (macOS `~/Library/Application Support/graphersal-memory`,
  Linux `$XDG_DATA_HOME/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`), not inside
  your repository.
- **One shared store by default.** Every memory carries its project, and search defaults to the
  current project plus your global memory: preferences and cross-project rules. The global
  memories live in the same store, without a project.
- **The settings file** (`config.json` in the data directory, written with the defaults by
  `install`): `{"version": 1, "daemon": {"idle": "2h"}, "summaries": {"enabled": false,
  "harness_command": "claude -p --model sonnet", "monthly_budget_usd": 5}}` (plus the optional
  `summaries.since`, a date). A missing file means the defaults; keys this version does not know
  are ignored, and `doctor` names them.
- **The project records** (`projects.json` in the data directory): every project that connected,
  with its git remote URL (the `origin` remote, else the first one, without credentials) and when it
  was first seen. The project key stays the canonical root path, so moving a repository orphans its
  memories for now; the recorded remote lets a later version find the moved repository and re-map
  them. The daemon records a project the first time it connects in the daemon's life.
- **The store registry** (`registry.json` in the data directory) says which store serves which
  project: `{"version": 1, "default": "default", "projects": {}}`. Today every project goes to
  the default store; the registry is where isolated projects will be listed.
- **Isolated projects** (planned): a project can get a store of its own, for example for a client
  under NDA, for handing a project to a team, or for an experiment. Your global memory is copied
  into it one way. Every MCP connection goes to exactly one store, and nothing references across
  stores.
- Background work (backups, checkpoints, session indexing, summaries, imports) runs off the thread
  that answers the agent, so a long job never makes a memory lookup wait. The daemon ticks each
  store's scheduled jobs and runs them on worker threads of their own; the session indexing runs
  on a thread of its own and writes in small units (see below).

## Past sessions

- **Which sessions.** Every Claude Code transcript `~/.claude/projects/<project>/<session>.jsonl`
  that is finished: unchanged for 30 minutes, or named by the `SessionEnd` hook. The transcript a
  running session writes is left for later, and the transcripts of sub-agents (in
  `<session>/subagents/`, and sub-agent records inside a transcript) are never indexed.
- **What is kept.** A `session` vertex (`session:<transcript name>`: the title Claude Code
  generated, start and end, branches, the files edited relative to the project, the number of
  turns, Claude Code's own compaction summaries) and one `turn` per prompt
  (`turn:<session>:<n>`: the prompt with the messages sent while the assistant worked and the
  answers to its questions, the assistant's text replies until the next prompt, the names of the
  tools it used). Tool inputs and results are not kept. Every vertex carries the session's
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

## Profiles: the two MCP servers

| Server | Default | Tools | Enforcement |
|---|---|---|---|
| `use` | on | search and read memories, remember, forget, search and read past sessions | the engine's access policy allows only what these tools write |
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
version, so edit copies under another name. Each memory function that reads or writes memories
takes the project as a parameter. The server sets it on every call to the connection's project and
hides it from the tool list, so an agent cannot read another project's memories by passing a
different one.

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
