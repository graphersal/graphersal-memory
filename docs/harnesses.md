# Supported harnesses

> Pre-release: Claude Code is supported by `install`, `uninstall`, `status`, `doctor` and
> `import` (stage M3), the indexing of its past sessions with opt-in summaries (stage M4) and the
> A/B `bench` (stage M5); opencode 1.x by all of them since stage M7a, opencode 2.x since M7b.
> Both harnesses can be installed at once
> and share one store and one daemon. The other harnesses are targets, listed with what each one
> needs.

Every harness below supports MCP, so the memory tools work the same way everywhere. What differs is
how graphersal-memory is installed into the harness, where the harness keeps its rules and
transcripts, and whether it has hooks.

| Harness | Status | MCP registration | Rules exported to | Past sessions from | Hooks |
|---|---|---|---|---|---|
| Claude Code | install, import, hooks, rule block (M3); past sessions, opt-in summaries (M4); bench (M5) | a member of `mcpServers` in `~/.claude.json` (user scope), edited in place with a backup | a marked block in `~/.claude/CLAUDE.md` | local transcripts (`~/.claude/projects/`) | SessionStart / SessionEnd |
| opencode 1.x and 2.x | install, rule file, session plugin, past sessions, opt-in summaries, bench (M7a: 1.x; M7b: 2.x) | 1.x: members of `mcp`, 2.x: members of `mcp.servers`, in `~/.config/opencode/opencode.json(c)` (JSONC kept), edited in place with a backup | a rules file of our own, listed in `instructions` | its SQLite database (`~/.local/share/opencode/opencode.db`, both lines' tables), read-only | one plugin file for both lines: a session's start, and shutdown |
| OpenAI Codex CLI | planned (M7) | its MCP config | `AGENTS.md` (to be checked) | its local store (format to be checked) | to be checked |
| Cursor | planned (M7) | its MCP config | Cursor rules (to be checked) | its local store (format to be checked) | to be checked |

Where a harness has no hooks, synchronisation runs from the MCP server itself or on demand.

## Rules for every harness

- **Only managed parts are written.** That means a marked block in the instruction file, and
  entries recognisable as ours. Your own text and entries are never touched.
- If you edit inside the managed block by hand, the edit is reported as a conflict. It is never
  silently overwritten (and `uninstall` leaves such a block for you to remove).
- A block an earlier version wrote and nobody edited is updated by `install` and removed by
  `uninstall`, like the current one: each version knows the fingerprints of the earlier block
  texts. A block of a version it does not know (a newer one) is kept by `install` and reported.
- `uninstall` removes exactly the managed parts and leaves the rest as it was.

## Claude Code

Claude Code keeps its files in `~/.claude` (or in `$CLAUDE_CONFIG_DIR` when that is set; the user
configuration is then `$CLAUDE_CONFIG_DIR/.claude.json` instead of `~/.claude.json`).

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.claude.json` | `mcpServers.graphersal-memory`: `{"type": "stdio", "command": "<this binary>", "args": ["mcp", "--profile", "use"], "env": {}}` | its name, and a command named `graphersal-memory` with `mcp` as first argument |
| `~/.claude.json` | only with `install --enable-develop`: `mcpServers.graphersal-memory-develop` (`--profile develop`) | the same |
| `~/.claude/settings.json` | one group in `hooks.SessionStart` and one in `hooks.SessionEnd`, each with the single command `"<this binary>" hook session-start` (or `session-end`), timeout 30 seconds | a command that runs `graphersal-memory ... hook session-...` |
| `~/.claude/settings.json` | unless you keep it on (`--keep-auto-memory`): `"autoMemoryEnabled": false`, which switches Claude Code's own auto-memory off | its value; what it was before is recorded in `<data dir>/install/claude-code.json` |
| `~/.claude/CLAUDE.md` | the managed rule block with the memory language rule, appended after your own text | the lines `<!-- graphersal-memory:begin fingerprint=... -->` and `<!-- graphersal-memory:end -->` |

When the data or run directory was chosen explicitly (`--data-dir`, `GRAPHERSAL_MEMORY_DIR`,
`--run-dir`, `GRAPHERSAL_MEMORY_RUN_DIR`), the entries and hook commands carry it as
`--data-dir` / `--run-dir`.

- **In place, with a backup.** The JSON files are edited as text: only our members are inserted,
  in the file's own indentation, and every other byte stays. Before its first change, each file is
  copied to `<data dir>/install/backups/<time>/`, and `<data dir>/install/claude-code.json` records
  what changed. `claude mcp add` is not used: it rewrites the whole file (reordering and adding
  keys), so the file could not be given back byte for byte.
- **The develop server is off by not being registered.** Claude Code has no switched-off state for
  a user-scope MCP server (it can only disable a server per project, in each project's settings),
  so `install` leaves `graphersal-memory-develop` out. `install --enable-develop` registers it, and
  `uninstall --disable-develop` unregisters it; both leave everything else as it is.
- **An entry of another program under our name** (`mcpServers.graphersal-memory` that does not
  start this binary) is a conflict: it is kept, and the report says how to remove it.
- **Claude Code's auto-memory.** Claude Code 2.1 writes memory files of its own and loads them
  into every session; the `/memory` toggle stores the switch as `autoMemoryEnabled` in
  `~/.claude/settings.json` (Claude Code docs, "How Claude remembers your project", section
  "Enable or disable auto memory"; `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` is the environment
  variant, which install does not use). Install sets it to `false` by default, edited in place
  like the hooks. Nothing is lost: `uninstall` writes every memory back to the memory files and
  puts the setting back to what it was (removes it when it was not set), and an untouched
  settings file gets its original bytes back. `install --keep-auto-memory` (or "No" in the
  wizard) leaves it on, and on an installed system undoes install's own switch-off.
- **Restart running sessions** after install: Claude Code reads its configuration when a session
  starts. If Claude Code rewrote `~/.claude.json` while install ran and dropped the entry,
  `doctor` says so; run install again.

### The hooks

- `hook session-start` imports the memory files of the session's project that are new or changed
  since their last import (the project's memory directory under `~/.claude/projects/`), then
  prints one line, which Claude Code adds to the session's context: where the memory is and which
  tools to use.
- `hook session-end` does the same import and prints nothing.
- Both stop after 15 seconds, exit with code 0 on every error (a note on stderr), and skip the
  import when another import is running.
- Both then queue the indexing of finished sessions in the daemon (one control line, answered at
  once): `session-start` marks its own transcript as still being written, `session-end` marks it
  as finished. The indexing and the opt-in summaries run in the daemon, never in the hook. With
  `GRAPHERSAL_MEMORY_HOOK=off` (set for the summaries' own `claude -p` runs) a hook does nothing.

### The import of the file memory

`install`, `import` and the hooks read `~/.claude/projects/<project>/memory/`: the `MEMORY.md`
index and one markdown file per memory with the front matter `name`, `description` and
`metadata.type`.

| From | To |
|---|---|
| the file name | the slug (lower case; other characters become `-`) |
| `metadata.type` (or a top-level `type`) | the kind: `user`, `feedback`, `project`, `reference`; missing or unknown becomes `project` (counted in the report) |
| `description` | the summary (else the first line of the text) |
| the text below the front matter | the body; a trailing `**Why:**` / `**How to apply:**` section is kept as the memory's why and how to apply |
| its `MEMORY.md` line `- [Title](file.md) — hook` | title, hook and the order of the list |
| `[[slug]]` in the text | links to those memories |
| the project directory | the project: the `cwd` recorded in the project's transcripts, else the directory name decoded against the file system; the memories belong to that directory's git root, as a session started there would. A directory whose project no longer exists is skipped and reported. |

Every memory is written with `remember` (replace by slug, never a duplicate), and the fingerprint
of every imported file is kept in `<data dir>/import/claude-code.json`, so a second run sends only
new and changed files. A memory you forget in the graph is not brought back while its file stays
unchanged. The files are only read. Rules in `CLAUDE.md` files are not imported yet.

### `uninstall`

- First the export: every memory goes back into the file memory, per project
  `~/.claude/projects/<project>/memory/MEMORY.md` and `<slug>.md` (the format of the import
  above, so a later import gives the same memories back), the global memories into
  `~/.claude/rules/graphersal-memory-global.md`. Claude Code loads every file of the user rules
  directory `~/.claude/rules/` in every project, and nobody else writes this file; the managed
  block in `CLAUDE.md` is not used, because uninstall removes it. A file that already holds the
  same text is left alone; a memory file that differs is kept and the graph's version goes beside
  it as `<slug>.graphersal.md`; a `MEMORY.md` that differs keeps every line and gets only the
  index lines of the memories it does not list yet appended (in the format the import reads;
  user decision 2026-11-10); nothing is deleted. In a terminal uninstall asks first ("Export N
  memories of M projects back to Claude Code's file memory? [Y/n]"); `--yes` exports,
  `--no-export` skips it. If the memories cannot be read, nothing is removed.
  `graphersal-memory export --to-files [--project ROOT]` runs the same export alone.
- A file nobody changed since install wrote it gets the bytes of its backup back, or is deleted
  when install created it. Directories install created are removed when they are empty again.
- A file changed since (Claude Code rewrites `~/.claude.json` all the time) keeps every change:
  only our members, hook groups and block are removed, plus the containers install created and
  that are empty now.
- `--purge` also stops the daemon and deletes the data directory with every memory in it, after a
  confirmation (or `--yes`). It refuses a directory that does not look like a graphersal-memory
  data directory.

### `status` and `doctor`

`status` shows the daemon, the data directory, the settings, and for Claude Code each part
(installed, outdated, missing, conflict), how many memory files were imported, the auto-memory
state, the memory language and the embedding model (name, size, licence; whether it is
downloaded). `doctor` also checks that the configured model is downloaded and loads, and notes
projects that look like a moved one (`projects merge`). `doctor` checks
the same parts plus a missing binary behind a registration, a second registration of the server at
local scope, the settings file, the registry, the socket path length, the daemon and memory files
not imported yet, and names a fix for each problem (exit code 1 when it found one).

### `bench` and `claude -p`

The summaries and `bench` run your harness command (`summaries.harness_command` in
`config.json`, `claude -p --model sonnet` by default; opencode's command when it is installed
alone, see below). With `claude` `bench` builds
builds each arm from Claude Code's flags (`--strict-mcp-config --mcp-config`, `--setting-sources
""` with `--settings`, `--tools`, `--allowedTools`, `--append-system-prompt-file`,
`--no-session-persistence`, `--json-schema`, `--max-budget-usd`), so it needs a recent Claude
Code. On Windows, npm installs `claude` as `claude.cmd`; graphersal-memory finds it through
`PATH` and `PATHEXT` and keeps line breaks out of its arguments (the prompts go on stdin, the
context in a file). See [Measuring it](ab-testing.md) for the method.

## opencode

Both release lines are supported: opencode 1.x (verified against the source of v1.19.5,
github.com/anomalyco/opencode, formerly sst/opencode; smoke-tested with 1.18.35) and opencode 2.x
(verified against the source of v2.2.4; smoke-tested with 2.2.4). OpenCode 2 is largely a
rewrite, but it keeps the configuration directory, the plugin directory, `instructions` and the
session database file; what differs is listed below and handled by graphersal-memory.

### Which line is installed

`install`, `uninstall`, `status` and `doctor` ask `opencode --version` (the `opencode` on your
`PATH`, else `~/.opencode/bin/opencode`, where opencode's install script puts it). opencode
creates its directories even for `--version`, so it is asked with a home and XDG directories of
its own in a temporary directory, which is removed afterwards. Without the binary, opencode's
database says it: 2.x's tables mean 2.x has run (it upgrades the file in place), 1.x's tables
alone mean 1.x. With neither, the current line (2.x) is assumed. `status` and `doctor` show the
result and where it came from (`version: opencode 2.x (2.2.4, from `.../opencode --version`)`).
The hooks do not ask: they only need the database.

### What differs between 1.x and 2.x

| | opencode 1.x | opencode 2.x |
|---|---|---|
| configuration files read in `~/.config/opencode/` | `config.json`, `opencode.json`, `opencode.jsonc` ([v1.19.5 `config/config.ts`](https://github.com/anomalyco/opencode/blob/v1.19.5/packages/opencode/src/config/config.ts)) | `opencode.json`, `opencode.jsonc` only; `$OPENCODE_CONFIG_DIR` replaces the directory ([v2.2.4 `config/discovery.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/config/discovery.ts), [`util/src/global.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/util/src/global.ts)) |
| a local MCP server | `mcp.<name>`: `{"type": "local", "command": [...], "enabled": true}` | `mcp.servers.<name>`: `{"type": "local", "command": [...], "disabled": false}` ([`schema/src/mcp.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/schema/src/mcp.ts)); the 1.x shape is still read as a legacy entry ([`config/normalize.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/config/normalize.ts), `normalizeMcp`) |
| MCP tools | direct tools named `<server>_<tool>` | the same names ([`tool/mcp.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/tool/mcp.ts)), but by default reachable only as JavaScript inside its `execute` tool ("Code Mode", [`codemode/instructions.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/codemode/instructions.ts)) unless the server sets `"codemode": false` |
| rules | the first of `~/.config/opencode/AGENTS.md` and `~/.claude/CLAUDE.md`, plus every file in `instructions` | `~/.config/opencode/AGENTS.md` and the project's `AGENTS.md` files (no `CLAUDE.md` fallback), plus every file in `instructions` ([`config/plugin/instruction.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/config/plugin/instruction.ts)) |
| plugins | every `{plugin,plugins}/*.{js,ts}` of the configuration directory; a function export, or a default export with `server` | the same directories ([`plugin/source-directory.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/plugin/source-directory.ts)); a default export with `id` and `setup(ctx)` ([`plugin/module.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/plugin/module.ts), [Promise plugin API](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/plugin/src/README.md)) |
| session events a plugin sees | `session.created`; `dispose` at shutdown; a TUI toast | `ctx.event.subscribe`: `session.created` (not seen for a session `opencode run` creates), `session.execution.started` per prompt; the cleanup `setup` returns; no toast for server plugins |
| sessions | tables `session`, `message`, `part` in `opencode.db` | tables `session_v2`, `session_message`, `session_part` in the same file ([`session/sql.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/session/sql.ts)); an upgrade copies every 1.x session with its id and keeps the 1.x rows ([`database/v1-migration.bun.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/database/v1-migration.bun.ts)) |
| headless run | `opencode run --model <provider/model> --format json` | the same flags and the same JSON event lines (`text`, `step_finish` with `cost` and `tokens`, `error`; [`cli/src/run/noninteractive.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/cli/src/run/noninteractive.ts)), but `run` talks to a shared background server unless given `--standalone` ([`services/server-connection.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/cli/src/services/server-connection.ts)) |
| logins | `~/.local/share/opencode/auth.json` | imported from `auth.json` into the database once ([migration `import_legacy_credentials`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/database/migration/20260805200742_import_legacy_credentials.ts)); new logins live in the database only |
| memory feature of its own | none | none |
| free models | `opencode/big-pickle`, `opencode/*-free` | the same; `big-pickle` is still the first preferred free model ([`model.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/core/src/model.ts)) |

### Where opencode keeps things

- **Configuration**: `$XDG_CONFIG_HOME/opencode/`, else `~/.config/opencode/` (also on Windows,
  under the home directory; 2.x: `$OPENCODE_CONFIG_DIR` when set). All files are JSONC
  (comments, trailing commas). Install edits the first of the line's files that exists (1.x:
  `opencode.jsonc`, `opencode.json`, `config.json`; 2.x: `opencode.jsonc`, `opencode.json`) as
  text, keeping every comment, or creates `opencode.json` with only its `$schema`.
- **Rules**: both lines load every file listed in `instructions`. 1.x loads the first of
  `~/.config/opencode/AGENTS.md` and `~/.claude/CLAUDE.md` that exists, so creating `AGENTS.md`
  would make it stop loading a Claude Code user's `CLAUDE.md` without a word. Install therefore
  never touches `AGENTS.md`: the managed block goes into a file of its own,
  `~/.config/opencode/graphersal-memory.md`, listed in `instructions`. 2.x has no `CLAUDE.md`
  fallback, but the same file and the same entry work there, so nothing changes with the line
  (user decision, 2026-11-10).
- **Session start and end**: opencode has no hooks in its configuration, but both lines
  auto-load every `plugins/*.js` (and `plugin/`) file of the configuration directory. Install
  writes `plugins/graphersal-memory.js`, one file for both lines: its default export carries
  1.x's `server` and 2.x's `setup` (each line ignores the other's member), so it keeps working
  when opencode is upgraded. It starts `graphersal-memory hook session-start --harness opencode`
  (detached) when a session starts (1.x: `session.created`; 2.x: the session's first
  `session.execution.started`) and `session-end` for the sessions it saw when opencode shuts down
  (1.x `dispose`, 2.x the plugin's cleanup; neither line has a session-end event, and 2.x's
  background server may keep running between your sessions, so a session also counts as finished
  after 30 unchanged minutes). On 1.x the start hook's line is shown as a toast
  (`client.tui.showToast`); 2.x has no toast for server plugins, so it is not shown there. opencode
  has no file memory, so nothing is imported.
- **Sessions**: a SQLite database (`$XDG_DATA_HOME/opencode/opencode.db`, default
  `~/.local/share/opencode/`; `OPENCODE_DB` when set). It is opened read-only. 1.x keeps its
  sessions in the tables `session`, `message` and `part`; 2.x in `session_v2`, `session_message`
  and `session_part` of the same file. When 2.x upgrades a 1.x database it copies every session
  with its id and keeps the 1.x rows, so graphersal-memory reads both and takes each session
  once: from 2.x's tables when 2.x changed it after the copy, else from 1.x's (a session that was
  only copied is therefore not indexed or summarized again). Sub-agent sessions (`parent_id`) are
  left out; a session is finished when unchanged for 30 minutes or when the plugin's end hook
  names it. 2.x release builds use `opencode.db`; its other channels (beta, dev builds) keep
  their own `opencode-<channel>.db`, which is not read.
- **Memory**: neither line has a memory feature of its own, so there is nothing to switch off and
  no such wizard step.

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.config/opencode/opencode.json` (or the `.jsonc` there; 1.x also `config.json`) | 1.x: `mcp.graphersal-memory`: `{"type": "local", "command": ["<this binary>", "mcp", "--profile", "use"], "enabled": true}`; 2.x: `mcp.servers.graphersal-memory`: `{"type": "local", "command": [...], "disabled": false, "codemode": false}` | a command whose program is `graphersal-memory` with `mcp` first |
| the same | `graphersal-memory-develop` with `--profile develop`, switched off (1.x `"enabled": false`, 2.x `"disabled": true`; `--enable-develop` switches it on, `uninstall --disable-develop` off again) | the same |
| the same | the rules file's path as one element of `instructions` | its path |
| `~/.config/opencode/graphersal-memory.md` | the managed rule block (fingerprinted begin line; a hand edit is a conflict) | the marker lines |
| `~/.config/opencode/plugins/graphersal-memory.js` | the session plugin (both lines) | its first line |

On 2.x the entries set `"codemode": false`: without it, 2.x would let the agent reach the memory
tools only by writing JavaScript inside its `execute` tool. With it they stay direct tools named
`graphersal-memory_<function>`, as on 1.x and as the rule block names them, so a memory search
before an answer is one plain call.

Backups, the state (`<data dir>/install/opencode.json`), the byte-identical `uninstall` of an
untouched file and the structural removal from a changed one work as for Claude Code. Uninstall
removes our entries in either shape from every configuration file that holds them.

### Upgrading opencode from 1.x to 2.x

graphersal-memory keeps working right after the upgrade: 2.x still reads the 1.x shape of our
MCP entries, and the plugin file serves both lines. Two things are in the wrong place or shape,
though, and `doctor` reports them as outdated, naming the fix:

- entries install wrote into `config.json`, which 2.x no longer reads (the memory tools are then
  missing in 2.x);
- entries in the 1.x shape (`mcp.graphersal-memory`), which 2.x reads only as legacy
  configuration.

Run `graphersal-memory install` again: it removes our parts from `config.json` (the rest of the
file stays) and from `mcp.<name>`, and writes them as `mcp.servers.<name>` into `opencode.json`
(or `opencode.jsonc`). A later `uninstall` still gives every file back as it was before the first
install.

### `uninstall` and the export

opencode has no file memory to write the memories back into. Uninstall therefore exports every
memory into readable Markdown files under `~/graphersal-memory-export/opencode/` (per project
`<encoded root>/MEMORY.md` and one file per memory, `global.md` for the global ones; default yes,
nothing is deleted) and prints the path. The export is **not** loaded into opencode
automatically: listing it in `instructions` would load every memory into every session (the
growing context graphersal-memory exists to avoid) and would change opencode's configuration
after the uninstall that promises to give it back unchanged (user decision, 2026-11-10, for both
lines). The report says how to list a file yourself. `graphersal-memory export --to-files
--harness opencode` writes the same export at any time.

### Summaries and `bench` through `opencode run`

With opencode installed alone, install sets `summaries.harness_command` to `opencode run --model
opencode/big-pickle`, a model of opencode Zen that is free for a limited time and needs no login
(any `opencode run ...` command works). Every run gets `--format json` (one JSON event per line;
cost and tokens from the `step_finish` events, the same on both lines) and, on 2.x,
`--standalone` (asked from the program with `--version`): 2.x's `run` otherwise connects to its
shared background server, which has your configuration and your session database. Every run is
in a sandbox of its own: `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_STATE_HOME` point into the
data directory, so none of your configuration, plugins, rules or MCP servers are loaded and the
run is not stored in your session database; `OPENCODE_DISABLE_PROJECT_CONFIG=1`,
`OPENCODE_DISABLE_CLAUDE_CODE=1`, `OPENCODE_DISABLE_AUTOUPDATE=1`; every permission is denied
(the bench's graph arm allows only the read functions `graphersal-memory_memories`, `_memory`,
`_sessions`, `_session`, `_recall`; denying everything also switches 2.x's Code Mode off there);
a copy of your `auth.json` (your provider logins) is put into the sandbox, the original is never
changed. 2.x imports that file into the sandbox's new database, but logins you made in 2.x itself
are kept only in your 2.x database (2.x no longer writes `auth.json`) and are not copied yet: on
2.x, the summaries and the bench can use a free model or a provider whose login came from 1.x
(an open point). With a free model the estimates say $0.00; the monthly budget and the bench
budget still stop at what opencode reports. opencode Zen limits free use: in the 2.x smoke run
the free model refused further calls after a handful of runs within a day ("Free usage limit
reached"); such a summary is reported as failed and tried again at a later indexing run (at most
2 attempts until the session changes). The bench arms for opencode: none (no context, no tools),
files (your opencode instruction files: `AGENTS.md`, and on 1.x the `CLAUDE.md` it falls back
to; opencode has no memory files), graph (the use tools on a fork of the store). opencode has no
per-call cost cap like `--max-budget-usd`; the run stops when its spend reaches the budget.

## Without a terminal (later)

Analysts, testers and managers who use desktop apps such as Claude Desktop instead of a CLI agent
are a later audience. The core is designed now so that it doesn't assume a terminal, hooks, local
transcripts or code. Rules can reach the model through the MCP server's instructions. Past
conversations can come from the services' data exports. A desktop app with an install wizard is
planned.
