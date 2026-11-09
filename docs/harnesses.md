# Supported harnesses

> Pre-release: Claude Code is supported by `install`, `uninstall`, `status`, `doctor` and
> `import` (stage M3), the indexing of its past sessions with opt-in summaries (stage M4) and the
> A/B `bench` (stage M5). The other harnesses are targets, listed with what each one needs.

Every harness below supports MCP, so the memory tools work the same way everywhere. What differs is
how graphersal-memory is installed into the harness, where the harness keeps its rules and
transcripts, and whether it has hooks.

| Harness | Status | MCP registration | Rules exported to | Past sessions from | Hooks |
|---|---|---|---|---|---|
| Claude Code | install, import, hooks, rule block (M3); past sessions, opt-in summaries (M4); bench (M5) | a member of `mcpServers` in `~/.claude.json` (user scope), edited in place with a backup | a marked block in `~/.claude/CLAUDE.md` | local transcripts (`~/.claude/projects/`) | SessionStart / SessionEnd |
| opencode | planned (M7) | its MCP config | `AGENTS.md` (to be checked) | its local store (format to be checked) | to be checked |
| OpenAI Codex CLI | planned (M7) | its MCP config | `AGENTS.md` (to be checked) | its local store (format to be checked) | to be checked |
| Cursor | planned (M7) | its MCP config | Cursor rules (to be checked) | its local store (format to be checked) | to be checked |

Where a harness has no hooks, synchronisation runs from the MCP server itself or on demand.

## Rules for every harness

- **Only managed parts are written.** That means a marked block in the instruction file, and
  entries recognisable as ours. Your own text and entries are never touched.
- If you edit inside the managed block by hand, the edit is reported as a conflict. It is never
  silently overwritten (and `uninstall` leaves such a block for you to remove).
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
| `~/.claude/CLAUDE.md` | the managed rule block, appended after your own text | the lines `<!-- graphersal-memory:begin fingerprint=... -->` and `<!-- graphersal-memory:end -->` |

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
(installed, outdated, missing, conflict) and how many memory files were imported. `doctor` checks
the same parts plus a missing binary behind a registration, a second registration of the server at
local scope, the settings file, the registry, the socket path length, the daemon and memory files
not imported yet, and names a fix for each problem (exit code 1 when it found one).

### `bench` and `claude -p`

The summaries and `bench` run your harness command (`summaries.harness_command` in
`config.json`, `claude -p --model sonnet` by default). `bench` drives only `claude` for now: it
builds each arm from Claude Code's flags (`--strict-mcp-config --mcp-config`, `--setting-sources
""` with `--settings`, `--tools`, `--allowedTools`, `--append-system-prompt-file`,
`--no-session-persistence`, `--json-schema`, `--max-budget-usd`), so it needs a recent Claude
Code. On Windows, npm installs `claude` as `claude.cmd`; graphersal-memory finds it through
`PATH` and `PATHEXT` and keeps line breaks out of its arguments (the prompts go on stdin, the
context in a file). See [Measuring it](ab-testing.md) for the method.

## Without a terminal (later)

Analysts, testers and managers who use desktop apps such as Claude Desktop instead of a CLI agent
are a later audience. The core is designed now so that it doesn't assume a terminal, hooks, local
transcripts or code. Rules can reach the model through the MCP server's instructions. Past
conversations can come from the services' data exports. A desktop app with an install wizard is
planned.
