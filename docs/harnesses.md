# Supported harnesses

> Pre-release: Claude Code is supported by `install`, `uninstall`, `status`, `doctor` and
> `import` (stage M3), the indexing of its past sessions with opt-in summaries (stage M4) and the
> A/B `bench` (stage M5); opencode by all of them since stage M7a. Both can be installed at once
> and share one store and one daemon. The other harnesses are targets, listed with what each one
> needs.

Every harness below supports MCP, so the memory tools work the same way everywhere. What differs is
how graphersal-memory is installed into the harness, where the harness keeps its rules and
transcripts, and whether it has hooks.

| Harness | Status | MCP registration | Rules exported to | Past sessions from | Hooks |
|---|---|---|---|---|---|
| Claude Code | install, import, hooks, rule block (M3); past sessions, opt-in summaries (M4); bench (M5) | a member of `mcpServers` in `~/.claude.json` (user scope), edited in place with a backup | a marked block in `~/.claude/CLAUDE.md` | local transcripts (`~/.claude/projects/`) | SessionStart / SessionEnd |
| opencode (1.x) | install, rule file, session plugin, past sessions, opt-in summaries, bench (M7a) | members of `mcp` in `~/.config/opencode/opencode.json(c)` (JSONC kept), edited in place with a backup | a rules file of our own, listed in `instructions` | its SQLite database (`~/.local/share/opencode/opencode.db`), read-only | a plugin file: `session.created`, and shutdown (`dispose`) |
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

Verified against opencode 1.x (source of v1.19.5, github.com/anomalyco/opencode, formerly
sst/opencode; smoke-tested with 1.18.35). OpenCode 2 is a rewrite with other paths and is not
supported yet.

### Where opencode keeps things

- **Configuration**: `$XDG_CONFIG_HOME/opencode/`, else `~/.config/opencode/` (also on Windows,
  under the home directory). opencode merges `config.json`, `opencode.json` and
  `opencode.jsonc` from it; all are JSONC (comments, trailing commas). Install edits the first
  that exists (`opencode.jsonc`, `opencode.json`, `config.json`) as text, keeping every comment,
  or creates `opencode.json` with only its `$schema`.
- **Rules**: opencode loads the first of `~/.config/opencode/AGENTS.md` and
  `~/.claude/CLAUDE.md` that exists, plus every file listed in `instructions`. Creating
  `AGENTS.md` would make opencode stop loading a Claude Code user's `CLAUDE.md` without a word,
  so install never touches `AGENTS.md`: the managed block goes into a file of its own,
  `~/.config/opencode/graphersal-memory.md`, listed in `instructions`.
- **Session start and end**: opencode has no hooks in its configuration, but it auto-loads every
  `plugins/*.js` (and `plugin/`) file of the configuration directory. Install writes
  `plugins/graphersal-memory.js`, which starts `graphersal-memory hook session-start --harness
  opencode` (detached) on `session.created` and `session-end` for the sessions it saw when
  opencode shuts down (opencode has no session-end event; `dispose` is the nearest). The start
  hook's line is shown as a toast (`client.tui.showToast`): opencode has no status line a plugin
  can set. opencode has no file memory, so nothing is imported.
- **Sessions**: a SQLite database (`$XDG_DATA_HOME/opencode/opencode.db`, default
  `~/.local/share/opencode/`; `OPENCODE_DB` when set), tables `session`, `message` and `part`.
  It is opened read-only; sub-agent sessions (`parent_id`) are left out; a session is finished
  when unchanged for 30 minutes or when the plugin's end hook names it.
- **Memory**: opencode has no memory feature of its own, so there is nothing to switch off and no
  such wizard step.

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.config/opencode/opencode.json` (or the `.jsonc`/`config.json` there) | `mcp.graphersal-memory`: `{"type": "local", "command": ["<this binary>", "mcp", "--profile", "use"], "enabled": true}` | a command whose program is `graphersal-memory` with `mcp` first |
| the same | `mcp.graphersal-memory-develop` with `--profile develop` and `"enabled": false` (opencode has a switched-off state; `--enable-develop` sets `true`, `uninstall --disable-develop` sets `false` again) | the same |
| the same | the rules file's path as one element of `instructions` | its path |
| `~/.config/opencode/graphersal-memory.md` | the managed rule block (fingerprinted begin line; a hand edit is a conflict) | the marker lines |
| `~/.config/opencode/plugins/graphersal-memory.js` | the session plugin | its first line |

Backups, the state (`<data dir>/install/opencode.json`), the byte-identical `uninstall` of an
untouched file and the structural removal from a changed one work as for Claude Code.

### `uninstall` and the export

opencode has no file memory to write the memories back into. Uninstall therefore exports every
memory into readable Markdown files under `~/graphersal-memory-export/opencode/` (per project
`<encoded root>/MEMORY.md` and one file per memory, `global.md` for the global ones; default yes,
nothing is deleted) and prints the path. The export is **not** loaded into opencode
automatically: listing it in `instructions` would load every memory into every session (the
growing context graphersal-memory exists to avoid) and would change opencode's configuration
after the uninstall that promises to give it back unchanged. The report says how to list a file
yourself. `graphersal-memory export --to-files --harness opencode` writes the same export at any
time.

### Summaries and `bench` through `opencode run`

With opencode installed alone, install sets `summaries.harness_command` to `opencode run --model
opencode/big-pickle`, a model of opencode Zen that is free for a limited time and needs no login
(any `opencode run ...` command works). Every run gets `--format json` (one JSON event per line;
cost and tokens from the `step_finish` events) and runs in a sandbox of its own:
`XDG_CONFIG_HOME` and `XDG_DATA_HOME` point into the data directory, so none of your
configuration, plugins, rules or MCP servers are loaded and the run is not stored in your session
database; `OPENCODE_DISABLE_PROJECT_CONFIG=1`, `OPENCODE_DISABLE_CLAUDE_CODE=1`,
`OPENCODE_DISABLE_AUTOUPDATE=1`; every permission is denied (the bench's graph arm allows only the
read functions `graphersal-memory_memories`, `_memory`, `_sessions`, `_session`, `_recall`); a copy
of your `auth.json` (your provider logins) is put into the sandbox, the original is never changed.
With a free model the estimates say $0.00; the monthly budget and the bench budget still stop at
what opencode reports. The bench arms for opencode: none (no context, no tools), files (your
opencode instruction files, `AGENTS.md` or the `CLAUDE.md` it falls back to: opencode has no
memory files), graph (the use tools on a fork of the store). opencode has no per-call cost cap
like `--max-budget-usd`; the run stops when its spend reaches the budget.

## Without a terminal (later)

Analysts, testers and managers who use desktop apps such as Claude Desktop instead of a CLI agent
are a later audience. The core is designed now so that it doesn't assume a terminal, hooks, local
transcripts or code. Rules can reach the model through the MCP server's instructions. Past
conversations can come from the services' data exports. A desktop app with an install wizard is
planned.
