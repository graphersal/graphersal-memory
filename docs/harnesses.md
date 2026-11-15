# Supported harnesses

> graphersal-memory 0.2.1 supports Claude Code, opencode 1.x and 2.x, OpenAI Codex CLI, Cursor and
> CodeWhale (Cursor without summaries and `bench`, and checked against its documentation and
> fixtures only: a live check is pending; CodeWhale, new in 0.2.0, is a **preview**: built from its
> source (v0.10.2), tested against fixtures and run live once on macOS, details in its section).
> DeepSeek Harness (`dsh`, new after 0.2.1) is a **preview** too: built from its source (dsh-v0.2.2-rc.3) and
> tested against fixtures, not run live yet, without summaries and `bench`.
> Any of them can be installed at once and share one store and one daemon.

Every harness below supports MCP, so the memory tools work the same way everywhere. What differs is
how graphersal-memory is installed into the harness, where the harness keeps its rules and
transcripts, and whether it has hooks.

| Harness | Supported | MCP registration | Rule block (how to use the memory) | Past sessions from | Hooks |
|---|---|---|---|---|---|
| Claude Code | install, import, hooks, rule block, past sessions, opt-in summaries, bench | a member of `mcpServers` in `~/.claude.json` (user scope), edited in place with a backup | a marked block in `~/.claude/CLAUDE.md` | local transcripts (`~/.claude/projects/`) | SessionStart / SessionEnd |
| opencode 1.x and 2.x | install, rule file, session plugin, past sessions, opt-in summaries, bench | 1.x: members of `mcp`, 2.x: members of `mcp.servers`, in `~/.config/opencode/opencode.json(c)` (JSONC kept), edited in place with a backup | a rules file of our own, listed in `instructions` | its SQLite database (`~/.local/share/opencode/opencode.db`, both lines' tables), read-only | one plugin file for both lines: a session's start, and shutdown |
| OpenAI Codex CLI | install, rule block, hooks, past sessions, opt-in summaries, bench with a local model | one managed block of `[mcp_servers.*]` tables in `~/.codex/config.toml` (`$CODEX_HOME`), edited as text with a backup | a marked block in `~/.codex/AGENTS.md` (or `AGENTS.override.md` when that is the one Codex loads) | its rollout files (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`), read-only | SessionStart / SessionEnd in `~/.codex/hooks.json` (run once you trust them in `/hooks`) |
| Cursor | install, rule file, hooks, past sessions the hooks report (no summaries or bench through Cursor; not yet checked with a live Cursor) | a member of `mcpServers` in `~/.cursor/mcp.json`, edited in place with a backup | a rule file of our own, `~/.cursor/rules/graphersal-memory.mdc` (`alwaysApply: true`) | the transcripts its hooks name (`transcript_path`), read-only | sessionStart / sessionEnd in `~/.cursor/hooks.json` |
| CodeWhale (preview) | install, rule block, past sessions, the import of its own memory, opt-in summaries (no bench through CodeWhale yet; run live once, on macOS) | members of `servers` (or `mcpServers`) in `~/.codewhale/mcp.json`, edited in place with a backup; our tools in `[tools] always_load` of `~/.codewhale/config.toml` | a marked block in `~/.codewhale/AGENTS.md` (or the global file CodeWhale loads) | its saved sessions (`~/.codewhale/sessions/<id>.json`), read-only | none: the MCP connection marks the start and end |
| DeepSeek Harness (`dsh`; preview) | install, rule block, past sessions, other memory reported (no summaries or bench through dsh; not yet run live) | one managed block in `~/.dsh/cordis.patch.yml` (`$DSH_HOME`), the home layer every profile reads, edited as text with a backup | a marked block in `~/.dsh/AGENTS.md` | its session files (`~/.dsh/sessions/<project>/<session>/session.v4.jsonl.zstd`), read-only | none: the MCP connection marks the start and end |

The rule block tells the agent how to use the memory tools, what is worth saving (what later
sessions and similar tasks need, not the current task's plan or progress) and which memory
language to use; it holds none of your memories. The memories that always apply reach the agent through the start
context (below). Where a harness has no hooks, synchronisation runs from the MCP server itself or
on demand.

## Rules for every harness

- **Only managed parts are written.** That means a marked block in the instruction file, and
  entries recognisable as ours. Your own text and entries are never touched.
- If you edit inside the managed block by hand, the edit is reported as a conflict. It is never
  silently overwritten (and `uninstall` leaves such a block for you to remove).
- A block an earlier version wrote and nobody edited is updated by `install` and removed by
  `uninstall`, like the current one: each version knows the fingerprints of the earlier block
  texts. A block of a version it does not know (a newer one) is kept by `install` and reported.
- `uninstall` removes exactly the managed parts and leaves the rest as it was.

## The start context: what reaches the model at a session's start

Every pinned memory and every feedback and user memory (of the project and the global ones) goes
into the start of every session, grouped like `MEMORY.md` ("How to work", "About you",
"Pinned"), one line per memory, within `memory.start_context_chars` of `config.json` (default
8,000 characters; `0` = off). `graphersal-memory start-context` prints exactly that text. How it
gets to the model, per harness (documentation checked on 2026-11-11):

| Harness | Channel | Why this one |
|---|---|---|
| Claude Code | the SessionStart hook answers `{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": ...}}`; the MCP instructions of that connection leave it out | Claude Code adds `additionalContext` to the context "at the start of the conversation, before the first prompt" and does not show it as a chat message; it caps it at 10,000 characters (more is saved to a file and only a 2,000-character preview reaches the model), so the hook asks for at most that ([hooks](https://code.claude.com/docs/en/hooks)). Server instructions are cut at 4,096 characters (2,048 before v2.1.296; [MCP](https://code.claude.com/docs/en/mcp)), too little for the whole text. The MCP server knows its connection is Claude Code's (`CLAUDECODE`) and that our hook is in `settings.json`, and then leaves the text out of its instructions, so it is not sent twice. Without the hook, the instructions carry what fits 4,096 characters and name the rest. |
| Claude Desktop and other MCP clients without hooks | the MCP server's `instructions` | The MCP specification: instructions "MAY be added to the system prompt". |
| opencode 1.x and 2.x | the MCP server's `instructions`; the plugin still shows only the short start line as a toast | Both lines put every server's instructions into the system prompt (`<mcp_instructions>`: 1.x [`session/system.ts`](https://github.com/anomalyco/opencode/blob/v1.19.5/packages/opencode/src/session/system.ts), 2.x [`mcp/instructions.ts`](https://github.com/anomalyco/opencode/blob/dev/packages/core/src/mcp/instructions.ts)). A plugin could add context too (2.x `instructions.entry`), which would send it twice. |
| Codex | the MCP server's `instructions`; the SessionStart hook keeps printing the short start line | Codex gives a server's instructions to the model as the description of its tool namespace ([`codex-mcp/src/rmcp_client.rs`](https://github.com/openai/codex/blob/main/codex-rs/codex-mcp/src/rmcp_client.rs)); hooks run only after the user trusted them, so the instructions are the channel that always works. Its hook output would reach the model as well ("Plain text on `stdout` is added as extra developer context", [hooks](https://learn.chatgpt.com/docs/hooks)), so the hook does not repeat it. |
| Cursor | the sessionStart hook's `additional_context`, and the MCP server's `instructions` | `additional_context` is documented as "Additional context to add to the conversation's initial system context" ([hooks](https://cursor.com/docs/hooks)); whether Cursor gives MCP instructions to the model is not documented. Both carry it until a live check shows whether Cursor reads the instructions (then the text reaches the model twice; see [cursor-live-check.md](cursor-live-check.md)). |
| CodeWhale | the MCP server's `instructions`, cut to 4,096 characters (as Claude Code's without the hook) | CodeWhale gives the model a server's instructions as a runtime note, cut after 4,096 bytes (`crates/mcp/src/lib.rs`, `crates/tui/src/runtime_handoff.rs`); its hooks run only with `[hooks] enabled = true` and do not name the saved session, so install adds none. Our entry's `env` names the harness, so the server fits the text to that limit. |
| DeepSeek Harness (`dsh`) | the MCP server's `instructions`, cut to 4,096 characters (as Claude Code's without the hook) | dsh puts every server's instructions into the system prompt, but rejects the whole server (it does not cut) when they are longer than `maxInstructionBytes` (32,768 bytes by default; `packages/mcp/mcp-client/src/connection.ts`). Our row's `env` names the harness, so the server keeps the instructions far below that (a test proves the bound for any characters). |

**Claude Code without the hook needs v2.1.296 or newer** for the start context to arrive through
the instructions: graphersal-memory fits them to 4,096 characters, the limit since v2.1.296; older
versions cut server instructions at 2,048 characters, which the `use` instructions alone almost
fill (about 1,500), so little or none of the start context gets through. On an older version keep
the SessionStart hook installed (install adds it), or raise the limit with
`CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH` (v2.1.280 and newer; [MCP](https://code.claude.com/docs/en/mcp)).
With the hook, any version that supports SessionStart's `additionalContext` gets the whole text
(up to the hook's 10,000-character cap).

Not verified live: the Claude Code path is tested against the documented output contract, the
others against their documentation and source; no harness has been watched receiving it yet.

## Other memory beside graphersal-memory

Many machines already have a memory: the harness's own (Claude Code's auto-memory, Codex's
memories, Cursor's Memories), another memory tool (an MCP server such as mem0/OpenMemory,
claude-mem, Basic Memory, the reference `@modelcontextprotocol/server-memory`, Serena,
Supermemory, ByteRover/Cipher, Graphiti, Letta, cognee, Pieces, or a home-made one), an
instruction file that says where memory goes, or a program that writes the memory files
graphersal-memory imports. Two memories split the facts: the agent saves into one and later
searches the other. `install` (the wizard's step "Other memory" and the dry-run plan), `status`
and `doctor` show what is there; nothing of yours is changed unless you choose it.

**What is detected** (files only; no other server is ever started to find out):

| What | Where | How |
|---|---|---|
| Built-in memory | Claude Code `autoMemoryEnabled` and its memory files; Codex `[features] memories` and `$CODEX_HOME/memories_v2/`; Cursor's Memories | on/off and how many files; Cursor keeps its Memories itself (not readable from files: its Settings, "Rules & Memories", shows them); opencode and DeepSeek Harness have none |
| Other memory tools | the MCP entries of `~/.claude.json` (user scope and each project's local scope) and a project's `.mcp.json`, opencode's `mcp` / `mcp.servers`, Codex's `[mcp_servers.*]`, Cursor's `~/.cursor/mcp.json` and a project's `.cursor/mcp.json`, DeepSeek Harness's MCP client rows and `mcp-manager` servers in its home layer and each profile layer (`~/.dsh/profiles/*/cordis.patch.yml`) | a known tool's name in the entry's name, command, arguments or URL; or a name that says memory; or tool names that look like memory tools (`remember`, `recall`, `add_memory`, `search_memory`, `create_entities`, ...) where the configuration shows them: the entry's own allow lists, Claude Code's permission rules `mcp__<server>__<tool>` |
| Memory plugins | Claude Code `enabledPlugins` and `plugins/installed_plugins.json`, Claude Code hooks, opencode's `plugin` list and plugin files | a known tool's name |
| Instruction files | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md` and rules, `AGENTS.md` / `AGENTS.override.md`, opencode's `instructions`, Cursor rules (`*.mdc`, `.cursorrules`), DeepSeek Harness's `~/.dsh/AGENTS.md` and `~/.agents/AGENTS.md`, user scope and the current project's | names only, so it works in any language: a paragraph or list item (wrapped lines belong to it) naming `MEMORY.md`, a known memory tool, another memory server found on the machine, or a memory tool name such as `create_entities`; a paragraph naming graphersal-memory is not counted. Whether the line really sends memory elsewhere is judged by the agent: the rule block tells it to point such an instruction out to you. opencode also loads `CLAUDE.md` (yours and the project's) when it has no `AGENTS.md` of its own; Codex loads a project file named in `project_doc_fallback_filenames` |
| External writers | each imported memory directory | a generated-file header in its `MEMORY.md` ("generated by", "do not edit", "exported from", ...), or imported files changed while no Claude Code session of that project ran (their modification time outside every transcript's time span, after the oldest transcript) |

**Your choice per other memory tool** (wizard step "Other memory"; flags for scripts, each taking
the server's name and repeatable):

1. **graphersal-memory is the main memory** (`--other-read-only NAME`; the default for a known
   memory tool): the rule block gets one more line naming the tool read-only: "search it only
   when you need to, never write there; save with `remember` here". Its entry stays as it is.
2. **Switch it off** (`--disable-other NAME`, or the choice in the wizard; never the default, and
   `--yes` alone never does it): the entry is switched off in the harness's configuration, in the
   same write as graphersal-memory's own entries (same backup, same rollback, same refusal of a
   read-only or changing file, a symbolic link stays a link). Claude Code and Cursor have no
   switched-off state for a user-scope entry, so it is removed and kept in the install state;
   opencode gets `"enabled": false` (1.x) or `"disabled": true` (2.x); Codex gets
   `enabled = false`. DeepSeek Harness's servers are not switched off by install: `doctor` and
   the plan say how to do it by hand (see its section). `uninstall` switches it on again: a file nobody changed since gets its
   original bytes back; in a file the harness rewrote since, the entry is put back (or its
   switch back to what it was). An entry you put back or switched on yourself meanwhile is left
   as it is. `install --enable-other NAME` switches it on again without uninstalling.
3. **Keep both as they are** (`--keep-other NAME`; the default for a server recognised only by
   its name or its tools, which may do more than memory, such as a home-made knowledge base): the
   rule block does not name it.

A project's own registration (Claude Code's local scope, a project's `.mcp.json` or
`.cursor/mcp.json`) is shown, never changed; `doctor` says how to switch it off. Re-running
`install` keeps the earlier choices (`<data dir>/install/other-memory.json`).

**Import from another tool** where its format is known (default yes, `--no-import` skips it; the
files are only read), into global memories (those tools keep one memory for every project):

- the reference memory server: its knowledge graph file (`MEMORY_FILE_PATH` of the entry, else
  `memory.jsonl` or `memory.json` in npx's cache); each entity becomes the memory `kg-<name>`
  (observations as the body, relations as lines and links; kind `user` for an entity of type
  user/me/self/preference, else `reference`);
- Basic Memory: the project folders of `~/.basic-memory/config.json` (else `~/basic-memory`);
  each note becomes `basic-<permalink>`.

The prefixes keep an imported item from replacing a memory of yours. Later installs import only
new and changed items (`<data dir>/import/other-memory.json`). The other tools are listed as "not
importable yet". `uninstall`'s export leaves these memories out of the global file every session
loads (the other tool still has them; the graph keeps them too).

**Instruction files are never edited.** `doctor` shows the line with its file and line number and
the edit it suggests (delete it, or change it to "Memory lives in graphersal-memory ...; X is
read-only").

**Returning facts.** Every imported memory records its file and when it was imported. When an
import of a changed file brings back a memory that was forgotten in the graph, or one an agent
changed there since, the import, `status` and `doctor` name the file and the fix: edit or delete
the file, stop the program that writes it, or stop importing it with
`graphersal-memory import --ignore <file or directory>` (`--unignore` imports it again; the
memories already imported from it stay).

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
  answers with one line (where the memory is and which tools to use) and the start context
  ([above](#the-start-context-what-reaches-the-model-at-a-sessions-start)) as
  `additionalContext`, which Claude Code adds to the session's context without showing it in the
  chat; with no always-loaded memory it prints the plain line.
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
alone, see below; `bench --harness-command COMMAND` for one run). With `claude` `bench` builds
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

## OpenAI Codex CLI

Verified against the source of release `rust-v0.167.0`
([github.com/openai/codex](https://github.com/openai/codex/tree/rust-v0.167.0)) and its
documentation (developers.openai.com/codex, now served from learn.chatgpt.com/docs). Links below
point into that tag.

### Where Codex keeps things

- **Home**: `$CODEX_HOME` when set (it must exist), else `~/.codex`
  ([`utils/home-dir`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/utils/home-dir/src/lib.rs)).
  The user configuration is `config.toml` there (TOML). Codex writes into it itself: the trust of
  a project (`[projects."<path>"]`), the trust of a hook (`[hooks.state."<key>"]`), a model you
  pick.
- **MCP servers**: `[mcp_servers.<name>]` tables with `command`, `args`, `env`, `enabled`
  (default true; `false`: not started), `startup_timeout_sec` (default 10), `enabled_tools`,
  `default_tools_approval_mode`
  ([`config/src/mcp_types.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/config/src/mcp_types.rs)).
  The model sees a tool as `mcp__<server>__<tool>`, every character outside `[A-Za-z0-9_]`
  replaced by `_`
  ([`codex-mcp/src/mcp/mod.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/codex-mcp/src/mcp/mod.rs),
  `qualified_mcp_tool_name_prefix`): ours are `mcp__graphersal_memory__memories`, and so on. A
  tool marked read-only by its server (ours: `memories`, `memory`, `recall`, `sessions`,
  `session`) runs without asking; `remember` and `forget` ask for your approval the first time,
  as in Claude Code (install leaves Codex's default as it is, user decision 2026-11-10). To allow
  them for good, answer Codex's question with the option that always allows the tool: Codex then
  writes `[mcp_servers.graphersal-memory.tools.remember]` `approval_mode = "approve"` into
  `config.toml` (`persist_custom_mcp_tool_approval_with` in the same file). graphersal-memory
  keeps such approvals (inside or outside its managed block; they are not a conflict) and removes
  them with its servers at uninstall, so no server entry without a command is left behind
  ([`core/src/mcp_tool_call.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/core/src/mcp_tool_call.rs),
  `requires_mcp_tool_approval`).
- **Rules**: Codex loads one global instruction file, the first non-empty of
  `$CODEX_HOME/AGENTS.override.md` and `$CODEX_HOME/AGENTS.md`
  ([`codex-home/src/instructions/mod.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/codex-home/src/instructions/mod.rs)),
  then the project's `AGENTS.md` files from the repository root down to the working directory
  ([`core/src/agents_md.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/core/src/agents_md.rs)).
  There is no setting that adds a second global file (`developer_instructions` is a single
  string in `config.toml`, and a project's configuration replaces it), so the managed rule block
  goes into the global file Codex loads, between the same fingerprinted marker lines as in
  Claude Code's `CLAUDE.md`. Your text around it is never touched.
- **Session start and end**: lifecycle hooks (feature `hooks`, stable and on by default,
  [`features/src/lib.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/features/src/lib.rs))
  in `$CODEX_HOME/hooks.json`, the shape of Claude Code's settings. The input names the
  session's rollout file (`transcript_path`); plain stdout of `SessionStart` becomes model
  context; `SessionEnd` may run at most 3 seconds
  ([`hooks/src/events/session_end.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/hooks/src/events/session_end.rs)).
  **Codex needs your approval for the two hooks.** Open Codex, type `/hooks` and trust both
  graphersal-memory hooks (SessionStart and SessionEnd); Codex records the trust in `config.toml`
  (
  [`hooks/src/engine/discovery.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/hooks/src/engine/discovery.rs)).
  install does not approve its own hooks for you: that is your security decision (user decision
  2026-11-10). Without the approval the memory works as usual, but past sessions are indexed
  only after 30 minutes idle instead of at session end. install's plan and report, `status` and
  `doctor` show an `ACTION NEEDED` line until the approval is recorded.
- **Sessions**: one JSON Lines rollout file per session,
  `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread id>.jsonl`
  ([`rollout/src/recorder.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/rollout/src/recorder.rs)),
  lines `{"timestamp", "type", "payload"}`
  ([`history/src/rollout_payload.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/history/src/rollout_payload.rs)).
  The reader takes `session_meta` (thread id, working directory, git branch; a sub-agent's
  session is left out), the `event_msg` records `user_message` and `agent_message` (what the
  default history mode persists,
  [`rollout/src/policy.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/rollout/src/policy.rs);
  the `paginated` mode's `item_completed` records too), tool calls, `patch_apply_end` (files
  edited) and compaction summaries. The injected instructions and environment context are not
  the user's words and are left out. Rollouts that an off-by-default feature
  (`local_thread_store_compression`) compresses to `.jsonl.zst` after 7 days are not read.
  Sessions are `session:<thread id>` in the graph.
- **Memory**: Codex has a memory feature of its own (`[features] memories = true`, off by
  default; generated files under `$CODEX_HOME/memories_v2/`). When you switched it on, the
  wizard offers to switch it off (the auto-memory question; `--keep-auto-memory` keeps it). Only a
  plain `memories = true` line of `[features]` (or `features.memories = true`) is changed, in
  place; uninstall switches it on again.

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.codex/config.toml` | one block between `# graphersal-memory:begin fingerprint=...` and `# graphersal-memory:end` with `[mcp_servers.graphersal-memory]` (`command` = this binary, `args = ["mcp", "--profile", "use"]`, `startup_timeout_sec = 30`: the first connection may start the daemon) and `[mcp_servers.graphersal-memory-develop]` (`enabled = false` until `--enable-develop`), placed before your first table so your top-level keys stay top-level; and, only if you agreed, `memories = false` | the marker comments; a hand edit inside is a conflict |
| `~/.codex/hooks.json` | one group in `hooks.SessionStart` (timeout 30 s) and one in `hooks.SessionEnd` (3 s), each `"<this binary>" hook session-start\|session-end ... --harness codex` | the command |
| `~/.codex/AGENTS.md` (or `AGENTS.override.md`) | the managed rule block (the tool names `mcp__graphersal_memory__<function>`) | the marker lines |

The configuration is edited as text (`codex mcp add` would rewrite the file); every edit is checked
by parsing the result, and nothing else in the file changes. Backups, the state
(`<data dir>/install/codex.json`) and the byte-identical `uninstall` of an untouched file work as
for Claude Code. Codex usually writes into `config.toml` after install (the trust of the hooks,
of a project): then `uninstall` takes out our block (and switches the memories back), and Codex's
own additions stay. Should Codex's own rewrite of the file ever drop our marker comments, install
and uninstall still find our tables by their command.

### `uninstall` and the export

Codex has no memory files we could write the memories back into (its own memories are generated
state). Uninstall exports every memory as readable Markdown under
`~/graphersal-memory-export/codex/` (as for opencode: per project `MEMORY.md` and one file per
memory, `global.md`; default yes, nothing is deleted). It is not loaded into Codex
automatically; paste what you need into `~/.codex/AGENTS.md` or a project's `AGENTS.md`.
`graphersal-memory export --to-files --harness codex` writes it again.

### Summaries and `bench` through `codex exec`

With Codex installed alone, install sets `summaries.harness_command` to `codex exec` (any `codex
exec ...` command works, for example `codex exec --oss --local-provider lmstudio` for LM Studio
on `localhost:1234` with its default model `openai/gpt-oss-20b`,
[`lmstudio/src/lib.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/lmstudio/src/lib.rs)).
Every run gets `--json --ephemeral --skip-git-repo-check --sandbox read-only` (with `--oss
--local-provider X` also `-c model_provider="X"`, see the smoke run) and the prompt on stdin, in a `CODEX_HOME` of its own in the data directory: its `config.toml` has your model
settings (`model`, `model_provider`, `model_reasoning_effort`, `oss_provider`,
`model_providers` copied from yours), `approval_policy = "never"`, no hooks, no Codex memories, no
history, no project instruction files and every built-in tool switched off (shell, unified exec,
image viewing, web search, sub-agents, apps, plugins); a copy of your `auth.json` (your login) is
put there, the original is never changed; none of your MCP servers are loaded. `codex exec
--json` reports tokens (`turn.completed`) but **no cost**
([`exec/src/exec_events.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/exec/src/exec_events.rs)):
a summary on a local model (`--oss`) counts $0.00; on any other model the monthly budget counts
the pre-call estimate of each call. The bench needs a known cost to enforce its budget, so it
runs through Codex only with a local model (`--oss`) and refuses otherwise. Its arms for Codex:
none (no context), files (your Codex instruction files, `AGENTS.md`; Codex has no memory files),
graph (the read functions of a server over a fork of the store, `enabled_tools`, approved without
asking), each with its own `CODEX_HOME` and no built-in tools.

### Smoke run (2026-11-10)

Codex 0.167.0 in a temporary npm prefix, a throwaway home, LM Studio with
`qwen2.5-coder-14b-instruct` (`codex exec --oss --local-provider lmstudio -m
qwen2.5-coder-14b-instruct`):

- `codex mcp list` showed both servers (develop `disabled`); `codex exec --strict-config`
  accepted the configuration install wrote.
- Every `codex exec` session started our MCP server (the daemon logged a `use` connection per
  session), but the model wrote the tool call as JSON text instead of calling the tool, in all
  three tries with increasingly explicit prompts: a limitation of that model with Codex's tool
  format, not of graphersal-memory. `remember` and `memories` through Codex with a local model
  are therefore not shown yet (a stronger local model, such as `openai/gpt-oss-20b`, or an OpenAI
  model is the next try).
- `--oss --local-provider lmstudio -m <model>` alone failed with "This chat uses lmstudio, but
  your current provider is openai": headless runs now add `-c model_provider="<provider>"`.
- The four rollouts were indexed through the session end hook's command (the hooks themselves
  were not approved in this run): 4 sessions, their turns searchable with `recall`.
- One summary through `codex exec` with the local model: headline, decisions with why ("Releases
  go out on Fridays"), outcomes; cost $0.00.
- `uninstall --harness codex --yes` deleted the three files install had created; only Codex's own
  runtime files stayed in `~/.codex`. (The export itself is shown in
  [install.md](install.md#codex); this store had no memories.)

## Cursor

Checked against Cursor's documentation as of 2026-11-10 (links below). Cursor is not installed on
the machine this was built on and needs a login, so everything is tested against hand-built files
in the documented formats; [cursor-live-check.md](cursor-live-check.md) is the checklist for a
live run, and until it is done this section says what is expected, not what was seen.

### Where Cursor keeps things

- **Home**: `~/.cursor` (`%USERPROFILE%\.cursor` on Windows). No environment variable moves it;
  `CURSOR_CONFIG_DIR` moves only the CLI's `cli-config.json`
  ([CLI configuration](https://cursor.com/docs/cli/reference/configuration)).
- **MCP servers**: the global file `~/.cursor/mcp.json`, `{"mcpServers": {"<name>": {"type":
  "stdio", "command", "args", "env"}}}` ([MCP](https://cursor.com/docs/mcp), "Using mcp.json",
  "STDIO server configuration", "Configuration locations"). The file has no documented
  switched-off state: a server is switched off in the app (Customize, "Can I temporarily disable
  an MCP server?") or with `agent mcp disable`, which keep their own list
  ([CLI parameters](https://cursor.com/docs/cli/reference/parameters)). So, as for Claude Code,
  the develop server is not registered until `install --enable-develop`. Servers of the global
  file load without a per-workspace prompt ([CLI changelog](https://cursor.com/docs/cli/changelog),
  "Global MCP servers auto-approved"). **Cursor asks before each MCP tool call by default**
  ("Tool approval"); install does not change that (the approval is yours, as for Codex): allow
  the memory tools in the approval prompt or in your run mode if you do not want to be asked.
  The docs do not say how a tool is named to the model; the rule file names the tools by their
  plain names.
- **Rules**: project rules are `.mdc` files in `.cursor/rules` with `description`, `globs` and
  `alwaysApply` front matter ([Rules](https://cursor.com/docs/rules)). For the user level the
  help page says: "User rule files in `~/.cursor/rules` (Windows: `%USERPROFILE%\.cursor\rules`)
  stay on the machine and do not sync"
  ([Rules help](https://cursor.com/help/customization/rules), "Where are rules stored?"). The
  User Rules of Customize are stored in your Cursor account, not in a file; graphersal-memory
  never touches them. install writes its own file `~/.cursor/rules/graphersal-memory.mdc`
  (`alwaysApply: true` and the managed rule block) and edits no file of yours. The MCP server's
  instructions carry the same guidance (whether Cursor gives them to the model is not
  documented). Caveat: a [forum report](https://forum.cursor.com/t/rules-in-home-folder-cursor-rules-are-not-applied/147236)
  of December 2025 (Cursor 2.2.43) says files in `~/.cursor/rules` were not applied, and Cursor
  staff called it a likely bug; the live check asks whether the rule reaches the model. Cursor
  also reads a project's `AGENTS.md` and `CLAUDE.md`, but not `~/.claude/CLAUDE.md` (not
  documented), so Claude Code's rule block does not reach Cursor.
- **Session start and end**: hooks in `~/.cursor/hooks.json`, `{"version": 1, "hooks":
  {"sessionStart": [{"command", "timeout"}], "sessionEnd": [...]}}`
  ([Hooks](https://cursor.com/docs/hooks)). User hooks run from `~/.cursor/` and need no
  approval (only project hooks need a trusted workspace); Cursor reloads the file on save
  (restart Cursor if they do not load). Every agent hook gets `conversation_id`,
  `workspace_roots`, `cursor_version` and `transcript_path` (the conversation's transcript, null
  when transcripts are off); `sessionStart` adds `session_id` and may answer
  `{"additional_context": "..."}`, which Cursor adds to the conversation's initial context. Both
  are fire-and-forget: the chat never waits for them.
- **Claude Code's hooks in Cursor**: Cursor also runs the hooks of `~/.claude/settings.json`
  ([Third Party Hooks](https://cursor.com/docs/reference/third-party-hooks), "Include Third-Party
  Plugins, Skills, and Other Configs", on by default). With graphersal-memory installed for
  Claude Code, its hooks would run on Cursor's sessions too; they recognise Cursor's input
  (`cursor_version`) and do nothing, so a Cursor session is never imported or indexed as a Claude
  Code one (install it for Cursor to get Cursor's sessions).
- **Sessions**: Cursor keeps its chat history in an internal database whose format is not
  documented and changes between versions; graphersal-memory does not read it. The documented
  way in is the hooks' `transcript_path`, a JSON Lines file under `agent-transcripts/` (sub-agent
  transcripts under `agent-transcripts/<parent>/subagents/`, [CLI
  changelog](https://cursor.com/docs/cli/changelog) of 2026-10-17). Our hooks record each
  transcript with its workspace (`workspace_roots`) in `<data dir>/sessions/
  cursor-transcripts.json`; only those transcripts are indexed (once unchanged for 30 minutes,
  or at `sessionEnd`), as `session:<conversation id>`. So sessions from before install, or while
  the hooks did not run, are not indexed. The line format of a transcript is not documented
  either: the reader takes what it recognises (a `role` or `type` of `user`/`assistant`, the
  `text` parts of `message.content`, the prompt inside `<user_query>` tags, `tool_use` names)
  and skips the rest; a transcript it cannot read counts as empty and is not indexed. The live
  check confirms this.
- **Memory**: the documentation of 2026-11 describes no memory feature of Cursor's own. Cursor's
  earlier "Memories" lost its settings UI in 2.1 (forum answers of Cursor staff, for example
  [this thread](https://forum.cursor.com/t/custom-modes-and-memories-gone-in-2-1/143744)), and no
  file switches it, so the wizard has no step for it and nothing is changed. If you still have
  old ones, Cursor's command palette entry "Export memories" saves them as a rule file; to bring
  them into the graph, ask the agent to `remember` them.

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.cursor/mcp.json` | `mcpServers.graphersal-memory` (`{"type": "stdio", "command": <this binary>, "args": ["mcp", "--profile", "use"]}`); `graphersal-memory-develop` only with `--enable-develop` | the command |
| `~/.cursor/hooks.json` | one entry in `hooks.sessionStart` and one in `hooks.sessionEnd` (timeout 30 s), each `"<this binary>" hook session-start\|session-end ... --harness cursor`; a new file starts with `"version": 1` | the command |
| `~/.cursor/rules/graphersal-memory.mdc` | front matter (`description`, `alwaysApply: true`) and the managed rule block | the marker lines; a hand edit inside is a conflict; a file of another program under this name is a conflict and kept |

`mcp.json` and `hooks.json` are edited as text (comments kept); backups, the state
(`<data dir>/install/cursor.json`) and the byte-identical `uninstall` of an untouched file work as
for Claude Code. Files install created are deleted again (with `~/.cursor` itself when install
created it and nothing else is in it); servers and hooks you add later stay.

### `uninstall` and the export

Cursor has no memory files to write back into. Uninstall exports every memory as readable Markdown
under `~/graphersal-memory-export/cursor/` (per project `MEMORY.md` and one file per memory,
`global.md`; default yes, nothing is deleted). It is not loaded into Cursor automatically; paste
what you need into a project's `AGENTS.md` or into a rule file under `~/.cursor/rules`.
`graphersal-memory export --to-files --harness cursor` writes it again.

### Summaries and `bench`: not through Cursor

Cursor's CLI (`agent -p --output-format json`) reports no cost
([output format](https://cursor.com/docs/cli/reference/output-format)), needs its own login or a
`CURSOR_API_KEY` ([authentication](https://cursor.com/docs/cli/reference/authentication)), and
reads your `~/.cursor` (MCP servers, hooks) with no documented way to isolate a run. The monthly
budget could not be kept and a run could not be sandboxed, so summaries and `bench` refuse a
Cursor command. With Cursor alone, summaries stay off unless another harness's command
(`claude -p`, `opencode run`, `codex exec`) is installed and set as `summaries.harness_command`;
the raw turns are indexed either way.

## CodeWhale (preview)

[CodeWhale](https://github.com/Hmbown/CodeWhale) is an open-source terminal coding agent
(formerly "DeepSeek TUI"; DeepSeek, Claude, GPT or local models). Built against the source of
release **v0.10.2** (2026-10-24); each fact below names the file it comes from. CodeWhale moves
fast (daily commits, a rename in 2026); a later version may differ.

**Live run (2026-11-14, CodeWhale 0.10.2 from Homebrew, macOS arm64, `openai/gpt-oss-20b` in LM
Studio):** installed beside Claude Code and opencode on one store (their files untouched); the
MCP server connected (on the first start after install it showed "disconnected" once and was
connected by hand in `/mcp`; on every later start it connected by itself); the model listed the
memory tools and called `memories` and `recall` against the shared store; asked "How should I
write tests that check file sizes?" without a word about memory, it quoted the user's rule from
the start context (and then applied it poorly: a model's quality, not the delivery); a session was
indexed in the second CodeWhale exited; `status` showed every part installed. Not yet seen live:
a memory written through CodeWhale, the import of its own memory, summaries through `codewhale
exec`, Linux and Windows. [codewhale-live-check.md](codewhale-live-check.md) is the checklist.

### Where CodeWhale keeps things

- **Home**: `$CODEWHALE_HOME` when set, else `~/.codewhale` (the home directory being `HOME`, then
  `USERPROFILE`; `crates/paths/src/lib.rs`). Without `CODEWHALE_HOME`, a file that exists only
  under the old `~/.deepseek` is read from there, so install edits the file CodeWhale actually
  reads: a new `~/.codewhale/mcp.json` beside an old `~/.deepseek/mcp.json` would hide the old
  one.
- **MCP servers**: `mcp.json` in the home (`CODEWHALE_MCP_CONFIG`, `DEEPSEEK_MCP_CONFIG` or
  `mcp_config_path` in `config.toml` move it), strict JSON, the servers under `servers` or
  `mcpServers` (a file with both does not parse; install refuses it), each `{command, args, env,
  disabled}` (`crates/tui/src/mcp.rs`). The file must be a regular file: CodeWhale refuses a
  symbolic link (`doctor` says so). The model sees a tool as `mcp_<server>_<tool>` with the
  server's name unchanged, so ours are `mcp_graphersal-memory_memories` and so on.
- **Deferred tools**: CodeWhale loads every MCP tool only after the model searched for it
  (`tool_search`), unless its exact name is listed in `[tools] always_load` of `config.toml`
  (`crates/tui/src/core/engine/tool_catalog.rs`). Install lists our `use` tools there, so the
  memory is there from the first turn; the rule block also tells the model to `tool_search`
  `graphersal-memory` if the tools are missing. Listing them costs their descriptions on every
  turn.
- **The start context**: CodeWhale gives the model a server's MCP instructions as a marked,
  non-authoritative runtime note, cut after 4,096 bytes (`crates/mcp/src/lib.rs`,
  `crates/tui/src/runtime_handoff.rs`). Our entry sets `GRAPHERSAL_MEMORY_HARNESS=codewhale` in
  its `env`, so our server knows it runs under CodeWhale and cuts the start context to fit, as it
  does for Claude Code without its hook (characters are counted; text beyond ASCII may still be
  cut by CodeWhale). Seen live: the memories arrived and the model used them unasked. To see
  what a session got, look for `mcp_server_instructions` in its `~/.codewhale/sessions/<id>.json`.
- **Rules**: one global instruction file, the first that exists of `~/.codewhale/AGENTS.md`,
  `~/.agents/AGENTS.md`, `~/.deepseek/AGENTS.md` (then the same names `instructions.md`), always
  under the home directory, even with `CODEWHALE_HOME`; it is loaded before the project's
  `AGENTS.md` files (`crates/tui/src/project_context.rs`). **All instructions share one budget of
  48 KiB**, trimmed from the front: when a project's `AGENTS.md` files are large, the global file
  (and our rule block in it) is dropped first. `doctor` warns when the current project's files
  leave no room; the memory tools stay loaded through `always_load` either way.
- **Session start and end**: CodeWhale has `session_start`/`session_end` hooks, but they run only
  with `[hooks] enabled = true` (a `config.toml` without a `[hooks]` table runs no hook at all)
  and carry a per-launch id that is not the saved session's (`crates/tui/src/hooks/executor.rs`).
  So install adds no hooks. CodeWhale starts its stdio MCP servers with the TUI and keeps them
  for the whole run, across `/new` and `/resume` (`docs/MCP.md`, "Connection Lifecycle"), so the
  MCP connection does the hooks' work: at its start the indexing of finished sessions is queued;
  when CodeWhale exits it kills our server at once (seen live), so the daemon, which sees the
  connection close, indexes that project's CodeWhale sessions written meanwhile as ended. A
  session left behind (a crash, a second CodeWhale in the same project) is indexed once
  unchanged for 30 minutes.
- **Tool approvals**: CodeWhale asks before every call of a tool of an MCP server you
  configured; "Allow for this conversation" lasts for the run, per tool, so a session asks once
  for each memory tool it uses. In 0.10.2 nothing makes that permanent: the `allow` rules of
  `~/.codewhale/permissions.toml` apply to shell commands and file tools only
  (`crates/tui/src/core/engine/turn_loop.rs`; `/permissions` lists rules for MCP tools but they do
  not apply, seen live), and a tool's `readOnlyHint` counts only for reviewed plugins
  (`crates/tui/src/mcp.rs`). Full Access (Shift+Tab) asks for nothing at all, shell included;
  install changes none of this.
- **Local models**: CodeWhale's system prompt and tool list do not fit an 8,192-token context
  (LM Studio's default answered "the number of tokens to keep from the initial prompt is greater
  than the context length"); load the model with 32,768 or more
  (`lms load <model> --context-length 32768`).
- **Sessions**: one JSON document per session, `<home>/sessions/<uuid>.json`, rewritten whole
  after every turn (`crates/tui/src/session_manager.rs`). The reader takes the active branch of
  the session's journal (with its times; else the `messages`): your prompts, the assistant's
  text, tool names, files edited (`write_file`, `edit_file`, `apply_patch`), the memories our
  tools wrote and CodeWhale's compaction summaries; thinking, tool results and the runtime notes
  CodeWhale adds as user messages (`<codewhale:runtime_event ...>`) are not read.
  Sub-agents are not saved as sessions. A plain one-shot `codewhale exec` saves nothing.
- **Memory**: CodeWhale has its own memory, off by default (`[memory] enabled = true`, `[memory]
  backend = "native"`, or `CODEWHALE_MEMORY`/`DEEPSEEK_MEMORY`). Its store is SQLite,
  `~/.codewhale/memory/store.sqlite3` (the `MEMORY.md` paths its `docs/MEMORY.md` names are only
  labels in v0.10.2; `crates/runtime/src/native_memory.rs`). What the model writes there stays a
  candidate until you review it. When it is on, the wizard offers to switch it off (`enabled =
  false`; uninstall switches it on again). A `backend` or environment switch is not changed: the
  plan says how. Its reviewed entries (`active`, not hidden) are imported read-only as
  `codewhale-<title>-<id>` memories: global ones as global memories, a repository's ones into
  that project, found by the SHA-256 of the repository's `origin` URL as CodeWhale computes it.
  Entries of a repository graphersal-memory has not seen yet are left out until a session ran
  there. The store is never written.

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.codewhale/mcp.json` | `graphersal-memory` (`{"command": <this binary>, "args": ["mcp", "--profile", "use"], "env": {"GRAPHERSAL_MEMORY_HARNESS": "codewhale"}, "connect_timeout": 60}`) and `graphersal-memory-develop` with `"disabled": true` (`--enable-develop` switches it on), under the key the file uses | the command |
| `~/.codewhale/config.toml` | `[tools] always_load` with our tool names: a managed block when the file has no `tools` table, else our names appended to your list in place (your list recorded, restored by uninstall); and `[memory] enabled = false` only when you agreed | the marker lines; the recorded names |
| `~/.codewhale/AGENTS.md` (or the global file CodeWhale loads) | the managed rule block, after your own text | the marker lines; a hand edit inside is a conflict |

Backups, the state (`<data dir>/install/codewhale.json`) and the byte-identical `uninstall` of an
untouched file work as for Claude Code. CodeWhale edits `config.toml` itself (through
`toml_edit`, which keeps comments, so our marker lines stay); then uninstall takes out only our
block, our names and the memory switch, and CodeWhale's own settings stay.

### `uninstall` and the export

CodeWhale's own memory is a SQLite store whose entries need your review, so graphersal-memory does
not write into it. Uninstall exports every memory as readable Markdown under
`~/graphersal-memory-export/codewhale/` (per project `MEMORY.md` and one file per memory,
`global.md`). It is not loaded automatically; paste what you need into `~/.codewhale/AGENTS.md`
or a project's `AGENTS.md`. `graphersal-memory export --to-files --harness codewhale` writes it
again.

### Summaries through `codewhale exec`; `bench` planned

With summaries on and CodeWhale alone, they run through `codewhale exec --json --prompt-file -`
with your own provider and model. A one-shot run has no tools, fires no hooks and saves no session
(`crates/tui/src/exec_agent.rs`), so it runs with your configuration in
`<data dir>/sessions/harness`. CodeWhale reports tokens but no cost: each call counts the
estimate against the monthly budget, except with a local provider named in the command
(`codewhale exec --provider ollama`, also `vllm`, `sglang`, `ds4`, `lm-studio`), which counts $0.
`bench` refuses a CodeWhale command for now: runs with tools are saved as sessions in your
CodeWhale home, and a run in a home of its own would need a copy of your provider credentials.
With summaries through CodeWhale, run the bench through another harness:
`graphersal-memory bench --harness-command "claude -p"`.

## DeepSeek Harness (`dsh`; preview)

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) (`dsh`) is DeepSeek's own
agent harness (TypeScript on Cordis; a CLI with the profiles `acp`, `web`, `headless`, `sdk`,
`sdk-minimal`, and a Desktop app). It is not CodeWhale (formerly "DeepSeek TUI"), which has its
own section: CodeWhale's `~/.deepseek` directory and `DEEPSEEK_*` variables are never read for
dsh. Built against the source of tag **dsh-v0.2.2-rc.3** (commit 38a6a5c, 2026-11-13); each fact
below names the file it comes from in the module docs (`crates/graphersal-memory/src/harnesses/dsh.rs`).
dsh is a developer preview that announces breaking changes (its README), and its row ids change
between versions; a later version may differ.

**Not run live yet.** Everything below is tested against files in dsh's formats on temporary
homes; no test or person has run graphersal-memory with a real dsh.
[dsh-live-check.md](dsh-live-check.md) is the checklist that ends the preview.

### Where dsh keeps things

- **Home**: `$DSH_HOME` when set (empty counts as unset; a leading `~` is expanded), else
  `~/.dsh`, the same on every OS (no XDG; `packages/util/home-paths/src/index.ts`). A relative
  `DSH_HOME` is not followed by install (dsh would resolve it against its working directory).
  The second instruction root `$DSH_AGENTS_HOME` (else `~/.agents`) is read, never written.
- **Configuration layers** (later wins): the bundle's layers, each profile's
  `~/.dsh/profiles/<name>/cordis.patch.yml` (dsh's Settings rewrite it), the **home layer**
  `~/.dsh/cordis.patch.yml`, then `--patch` overlays. Every profile, Desktop's included, applies
  the home layer, and nothing in dsh writes it (`packages/boot/app-boot/src/profile-context.ts`,
  `apps/cli/src/profile-boot.ts`). `profiles/<name>/cordis.yml` is rewritten at every start.
  So install writes the home layer only, never a profile layer or `cordis.yml`.
- **Patch files**: a top-level YAML list of patch operations (js-yaml 4, the JSON schema plus
  the `!!js` tag; duplicate keys and unknown tags are errors). A new row must sit inside
  `- insert: [...]`: a bare `- id: x` only overrides an existing row and is skipped with a
  warning when there is none. A row is switched off by `disabled: true` beside its `id`. A
  missing file is no layer; **an empty or comments-only file stops every dsh start**; dsh's own
  empty template is a `[]` under a comment header (`vendor/include/src/patch.ts`,
  `vendor/loader/src/config/entry.ts`, `packages/boot/app-boot/src/index.ts`).
- **MCP servers**: one `@deepseek-ai/dsh-mcp-client` row per server (shipped with the CLI;
  `packages/mcp/mcp-client/src/index.ts`). There is no `dsh mcp add`. The model sees a tool as
  `mcp__<serverName>__<tool>`, every tool loaded up front, so ours are
  `mcp__graphersal-memory__memories` and so on. Without `startupTimeoutMs` dsh's start waits for
  a server forever, so our rows set 60 seconds (the first connection may start the daemon). dsh
  starts the server with its own environment minus every `DSH_*` variable and every name with
  `KEY`, `PASSWORD`, `SECRET` or `TOKEN`, then adds the row's `env`
  (`packages/subprocess/subprocess/src/index.ts`, `packages/mcp/mcp-client/src/transport.ts`).
  A second server of the same name is refused ("serverName ... is already in use"), also against
  a server of the web and Desktop profiles' MCP manager (the `mcp-manager` row) and against a
  row of the same id or `serverName` inserted in a profile layer: `doctor` names each (a note
  with the file and the row to remove).
- **The start context**: the server's instructions go into the system prompt; above
  `maxInstructionBytes` (32,768 bytes by default, the attribution line included) the server is
  rejected, not cut (`packages/mcp/mcp-client/src/connection.ts`). Our row's `env` sets
  `GRAPHERSAL_MEMORY_HARNESS=dsh`, so our server gives the short start context (at most 4,096
  characters of instructions, as for Claude Code without its hook), far below the limit
  whatever the characters; a test proves the bound and a round trip fills the budget.
- **Rules**: the first request carries `~/.dsh/AGENTS.md`, then `~/.agents/AGENTS.md`, then the
  project's `AGENTS.md`/`CLAUDE.md` chain, 64 KiB per chain
  (`packages/context/agent-instructions/README.md`, `packages/bundle/base/cordis.patch.yml`).
  The rule block goes into `~/.dsh/AGENTS.md`. When the user chain (`~/.dsh/AGENTS.md` and
  `~/.agents/AGENTS.md`) is over 64 KiB, dsh omits `~/.dsh/AGENTS.md` first, and the rule block
  with it: `doctor` notes it (headings and framing count too, so dsh may drop it a little
  earlier). The `sdk-minimal` profile loads no instruction files, so there only the
  MCP server's instructions reach the model; `doctor` notes it when that profile exists.
- **Session start and end**: dsh's hook bridges are experimental packages no bundle loads (no
  session end, an empty `transcript_path`), so install adds no hooks and `hook --harness dsh`
  does nothing. dsh keeps one MCP server process for its whole run, so the connection does the
  hooks' work: at its start the indexing of finished sessions is queued; when it closes, the
  daemon indexes that project's dsh sessions written meanwhile as ended. Other sessions are
  indexed once unchanged for 30 minutes. dsh's README says it starts a short probe of a stdio
  server before the server itself; that probe opens and closes a connection at dsh's start,
  before any session is written (not seen live yet).
- **Project scoping (a known limit)**: dsh gives the server no MCP roots and starts it in dsh's
  own working directory, and one server process serves every session of that dsh process
  (`connection.ts`, `transport.ts`). In `headless`, and in any profile started from the
  project's directory, the project is right. In `web`, Desktop and `acp`, where one dsh serves
  several workspaces, every session's memories and session start go to the project of the
  directory dsh was started in (for Desktop, wherever the app was launched). Start `dsh web`
  from the project you work in, or use `headless`, until this is solved.
- **Sessions**: `~/.dsh/sessions/<project key>/<session>/session.v4.jsonl.zstd` (the
  `sdk-minimal` profile writes `session.v4.jsonl` uncompressed, in the same place;
  `packages/session/session-persistence-jsonl/src/format.ts`). The `.zstd` file is a chain of
  zstd frames whose last one can be cut by a crash or a write in progress: it is read frame by
  frame, and a cut frame ends the read (at most 128 MiB decoded; a larger file fails with that
  reason). The project key in the directory name is lossy, so the
  project is the header's `cwd`. The reader takes your prompts (messages dsh injects are not
  yours and are left out), the assistant's text, tool names and the memories our tools wrote;
  sub-agent sessions and sessions without a working directory (`_no-cwd/`) give no turns. A
  forked session (`isSeeded` in its header) starts with a copy of its parent's events up to a
  `session/end-seed` marker: that part is the parent's and is left out. Older
  generations beside the v4 file (`session.jsonl`, `session.v1..v3.jsonl[.zstd]`) are not read,
  and a file of an unknown version is logged and indexed as empty (not failed) until it
  changes. Sessions are indexed as `session:dsh-<uuid>`.
- **Memory**: dsh has none of its own (its "memory" rows are in-memory SQLite indexes).
- **Other memory**: other memory tools' servers in the home layer and every profile layer
  (inserted MCP client rows by their `serverName`, and the MCP manager's servers) are detected
  and can be named read-only in the rule block, but install never switches one off: our block
  is rewritten whole on every install, and a server in a profile layer belongs to dsh's
  Settings. `doctor` (and the plan, when you ask to switch one off) says how to do it by hand (an override `- id: <its
  id>` with `disabled: true` in the home layer, outside our block; the row itself when it has no
  `id`; dsh's Settings for an MCP manager server). Desktop's own profile layer lives in a
  directory the Electron app manages, which was not traced, so its servers are not seen.
- **Program**: the npm package `@deepseek-ai/dsh` installed globally (`npm install -g
  @deepseek-ai/dsh`; on Windows npm's `dsh.cmd`, also looked for in `%APPDATA%\npm`), or the
  Desktop app's link (`/usr/local/bin/dsh` on macOS, a `dsh.cmd` directory on the Windows user
  `PATH`). `npx @deepseek-ai/dsh` leaves no program behind: such a configuration is a leftover,
  shown by `status` and never offered, and `doctor` says how to install dsh globally. Debian's
  and Ubuntu's `dsh` package (a distributed shell) has the same name: a found `dsh` counts only
  when its real path or its script text names DeepSeek's package (it is never run to find out),
  and `doctor` names a `dsh` it rejected.
- **Version managers (a known limit)**: a `dsh` installed through Volta or mise is a link to the
  manager's own shim binary, and asdf's shim script does not name the package, so neither is
  recognised as DeepSeek Harness: `install` does not offer dsh and `doctor` says its program is
  not found. Install dsh globally with npm (`npm install -g @deepseek-ai/dsh`), or run
  `graphersal-memory install --harness dsh`, which installs anyway after a warning that the
  program is not found (status, doctor and uninstall work on the installed parts all the same).
- **Privacy**: dsh's session log upload is on by default and sends memories our tools return to
  DeepSeek with official API requests. Switch it off in the home layer to switch it off in every
  profile: dsh's web Settings toggle writes only that profile's layer, and `status`/`doctor` say
  "off in profile(s) ... only; on in the others" then. See
  [privacy.md](privacy.md#deepseek-harness-dsh).

### What `install` writes

| File | What | How it is recognised |
|---|---|---|
| `~/.dsh/cordis.patch.yml` (the home layer) | one managed block holding one `- insert:` item with two rows: `graphersal-memory` (`name: "@deepseek-ai/dsh-mcp-client"`, `config: {serverName, transport: stdio, command: <this binary>, args: [mcp, --profile, use], env: {GRAPHERSAL_MEMORY_HARNESS: dsh}, startupTimeoutMs: 60000}`) and `graphersal-memory-develop` with `disabled: true` (`--enable-develop` switches it on); with `$DSH_HOME` set, both rows' `env` also carry `DSH_HOME` with its absolute path (dsh drops `DSH_*` from the server's environment, and our server needs it to find the sessions) | the marker comment lines with the block's fingerprint; a hand edit inside, or a row of another program named like ours, is a conflict |
| `~/.dsh/AGENTS.md` | the managed rule block, after your own text | the marker lines; a hand edit inside is a conflict |

The block is appended to your list of patch operations, as text: every other byte of the file
stays, and every edit is parsed again and must add exactly our item. A missing file is created
(uninstall deletes it again); dsh's empty template `[]` is replaced by the block, and uninstall
puts the `[]` back, so the file never ends up empty or comments-only (which would stop dsh). A
file dsh itself could not start with (a syntax error, a duplicate key, an unknown tag, a top
level that is not a list) is refused, naming the file, with nothing written; so is a top level
written as a non-empty flow list (`[...]`): write it as `- ...` lines first. Backups, the state
(`<data dir>/install/dsh.json`) and the byte-identical `uninstall` of an untouched file work as
for Claude Code; in a file changed since, uninstall removes only our block.

A `DSH_HOME` set or changed after install points dsh at a home without our rows: run
`graphersal-memory install --harness dsh` again with that `DSH_HOME`.

### `uninstall` and the export

dsh has no file memory. Uninstall exports every memory as readable Markdown under
`~/graphersal-memory-export/dsh/` (per project `MEMORY.md` and one file per memory,
`global.md`). It is not loaded automatically; paste what you need into `~/.dsh/AGENTS.md` or a
project's `AGENTS.md`. `graphersal-memory export --to-files --harness dsh` writes it again.

### Summaries and `bench`: not through dsh

`dsh --profile headless --json -` cannot run without tools (there is no flag, and the tool rows
change from version to version), reports no cost, so the monthly budget could not be kept, and
saves every run as a session. So a summaries harness command or a `bench --harness-command`
naming `dsh` is refused with that reason. With dsh alone, summaries stay off; with another
harness installed, they and the bench run through it (`summaries.harness_command` in
`config.json`). This is revisited when dsh gains a way to run without tools.

## Windows

Windows 10 version 1803 or later, x86_64. CI runs the whole test suite on Windows, including the
daemon and the install/hook/uninstall tests through the real binary; no harness has been run live
on Windows yet. The home directory is your profile directory (`%USERPROFILE%`), as the harnesses
themselves resolve `~` there (Node's and Bun's `os.homedir()`, Rust's `dirs::home_dir`); a `HOME`
variable set by Git Bash or Cygwin is used only when `USERPROFILE` is missing.

| Harness | Files on Windows | Hooks run in | Source |
|---|---|---|---|
| Claude Code | `%USERPROFILE%\.claude.json`, `%USERPROFILE%\.claude\` (or `%CLAUDE_CONFIG_DIR%`) | Git Bash; PowerShell when Git Bash is not installed. Our hooks use the exec form, which runs without a shell (see below) | [settings](https://code.claude.com/docs/en/settings) ("On Windows, `~/.claude` means `%USERPROFILE%\.claude`"), [hooks](https://code.claude.com/docs/en/hooks) ("Exec form and shell form") |
| opencode 1.x and 2.x | `%USERPROFILE%\.config\opencode\` and `%USERPROFILE%\.local\share\opencode\opencode.db` (the XDG layout, also on Windows; `XDG_CONFIG_HOME`/`XDG_DATA_HOME` move them) | the plugin starts the binary directly (no shell), without a console window | [1.x `global.ts`](https://github.com/anomalyco/opencode/blob/v1.19.5/packages/core/src/global.ts), [2.x `global-roots.ts`](https://github.com/anomalyco/opencode/blob/v2.2.4/packages/util/src/global-roots.ts) |
| Codex | `%USERPROFILE%\.codex\` (or `%CODEX_HOME%`, which Codex canonicalizes; so does install, so the hook trust keys Codex records match) | `cmd.exe /C` (`%ComSpec%`) | [`utils/home-dir`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/utils/home-dir/src/lib.rs), [`hooks/src/engine/command_runner.rs`](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/hooks/src/engine/command_runner.rs) |
| Cursor | `%USERPROFILE%\.cursor\` | PowerShell, so install writes a command PowerShell runs (see below) | [hooks](https://cursor.com/docs/hooks) ("On Windows, the desktop app runs hook commands through PowerShell") |
| DeepSeek Harness (`dsh`) | `%USERPROFILE%\.dsh\` (or `%DSH_HOME%`); the program is npm's `dsh.cmd` (also looked for in `%APPDATA%\npm`) or the Desktop app's | no hooks | [`home-paths`](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.2-rc.3/packages/util/home-paths/src/index.ts), [`command-manager-entry.ts`](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.2-rc.3/apps/desktop/src/command-manager-entry.ts) |

- **Hook commands.** A command whose program is a quoted path (`"C:\...\graphersal-memory.exe"
  hook ...`) runs in Git Bash and `cmd.exe`, but not in PowerShell, which reads a quoted string
  as a value. So install writes:
  - **Claude Code**: the exec form, `"command": "C:\...\graphersal-memory.exe", "args": ["hook",
    "session-start", ...]`. Claude Code starts it without any shell, so it runs with Git Bash,
    without Git Bash, and whatever the path holds (Claude Code 2.1.139 or later, which added
    `args`; [hooks](https://code.claude.com/docs/en/hooks), "Exec form and shell form").
  - **Codex and Cursor**: a command every shell runs as it is: `graphersal-memory hook ...` when
    `graphersal-memory` on your `PATH` is this binary; else the path with forward slashes and no
    quotes (`C:/Users/you/AppData/Local/Programs/graphersal-memory/graphersal-memory.exe hook
    ...`) when it holds only letters, digits and `_ - . / : ~`; else the folder's 8.3 short name
    (`C:/Users/ANNLEE~1/...`) when the drive keeps short names. The same rule applies to
    `--data-dir` and `--run-dir`. When none of these works, Codex keeps the quoted form (it runs
    hooks in `cmd.exe`, which accepts it) and Cursor PowerShell's `& '...'` form.
  - Running `graphersal-memory install` again replaces hooks an older version wrote; `uninstall`
    removes ours in every form.
- **Without Git Bash.** Claude Code runs shell hooks in PowerShell when it cannot find Git Bash.
  Hooks written by an older graphersal-memory (the quoted form) then do nothing; `doctor` finds
  Git Bash the way Claude Code does (`CLAUDE_CODE_GIT_BASH_PATH` from Claude Code's settings or
  your environment, when it names an existing `bash.exe` or `sh.exe`; then `C:\Program
  Files\Git` and `C:\Program Files (x86)\Git`; then the Git installation of the `git` on your
  `PATH`; WSL's `C:\Windows\System32\bash.exe` never counts; [Troubleshoot
  installation](https://code.claude.com/docs/en/troubleshoot-install), "Claude Code on Windows
  requires either Git for Windows (for bash) or PowerShell") and says what to do: run `install`
  again, or install Git for Windows, or set the variable. `status` shows one Git Bash line.
  Even when no hook runs, the memory works: the MCP server itself imports the project's memory
  files and queues the session indexing at every session start and end (see
  [How it works: without hooks](how-it-works.md#without-hooks)).
- **The daemon** listens on an `AF_UNIX` socket (`%LOCALAPPDATA%\graphersal-memory\run\daemon.sock`),
  which Windows has had since version 1803; it is started detached, without a console window.
- **Summaries and `bench`** find npm's `claude.cmd` through `PATH` and `PATHEXT`; a run that
  exceeds its time limit is ended with its child processes (`taskkill /T`), since `claude.cmd`
  runs `node.exe` under `cmd.exe`.
- **Recorded paths** (edited files in transcripts and opencode's database) are compared without
  case and with either separator, and stored relative to the project with `/`.

## Without a terminal (later)

Analysts, testers and managers who use desktop apps such as Claude Desktop instead of a CLI agent
are a later audience. The core is designed now so that it doesn't assume a terminal, hooks, local
transcripts or code. Rules can reach the model through the MCP server's instructions. Past
conversations can come from the services' data exports. A desktop app with an install wizard is
planned.
