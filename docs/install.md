# Install and uninstall: what you see

This page has two parts: how to get the binary, and what `graphersal-memory install` and
`uninstall` then show and write, captured for every harness.

## The binary

Every [GitHub Release](https://github.com/graphersal/graphersal-memory/releases) carries one
archive per platform, `graphersal-memory-v<version>-<platform>.tar.gz` (Windows: `.zip`), for
`linux-x86_64`, `linux-aarch64` (both static), `macos-arm64`, `macos-x86_64` and
`windows-x86_64`, each with `LICENSE`, `NOTICE` and `README.md`; the installers `install.sh` and
`install.ps1`; and `SHA256SUMS` over all of them.

The installers find the archive of your platform in `SHA256SUMS`, download it, refuse it when the
checksum does not match, and put the binary into `~/.local/bin` (Windows:
`%LOCALAPPDATA%\Programs\graphersal-memory`). `install.ps1` adds that directory to your user
`PATH` (the registry's `HKCU\Environment`, its other entries kept as they are, unexpanded
references included) and to the current window's, so `graphersal-memory` runs in that window
and in every new one; `-NoPathUpdate` (or `GRAPHERSAL_MEMORY_NO_PATH_UPDATE=1`) leaves `PATH`
alone. `install.sh` does not change your `PATH` and tells you when the directory is missing
from it. They change nothing else. `uninstall` keeps the binary and its `PATH` entry; to remove
the binary on Windows, delete the directory and its entry in your user `PATH` (Settings, "Edit
environment variables for your account").

```sh
curl -fsSL https://github.com/graphersal/graphersal-memory/releases/latest/download/install.sh | sh
# a given version or directory
curl -fsSL https://github.com/graphersal/graphersal-memory/releases/latest/download/install.sh | sh -s -- --version v0.3.0 --dir /usr/local/bin
```

```powershell
irm https://github.com/graphersal/graphersal-memory/releases/latest/download/install.ps1 | iex
# a given version or directory
& ([scriptblock]::Create((irm https://github.com/graphersal/graphersal-memory/releases/latest/download/install.ps1))) -Version v0.3.0 -Dir C:\Tools
```

The same options can be set as environment variables (`GRAPHERSAL_MEMORY_VERSION`,
`GRAPHERSAL_MEMORY_INSTALL_DIR`, on Windows also `GRAPHERSAL_MEMORY_NO_PATH_UPDATE`); `--help` lists them. A shell under Rosetta 2 on Apple silicon
gets the native arm64 build.

Rather check first? Download the archive and `SHA256SUMS` from the release, compare the checksum,
verify the build provenance with the GitHub CLI
(`gh attestation verify <archive> -R graphersal/graphersal-memory`), and put `graphersal-memory`
on your `PATH` yourself.

From source, with Rust 1.88 or newer and a C compiler (for the bundled SQLite and `ring`):

```sh
cargo install --locked graphersal-memory
```

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), from 0.3.0 on: the
release's build into cargo's directory, without building (`[package.metadata.binstall]` in the
crate names the archives; a platform without a release build is built as with `cargo install`):

```sh
cargo binstall graphersal-memory
```

## Upgrade

`graphersal-memory upgrade` asks GitHub for the latest release (the redirect of
`releases/latest`), prints the installed and the latest version, and the plan:

```text
graphersal-memory upgrade
  installed: 0.2.0 (/Users/you/.local/bin/graphersal-memory)
  latest:    0.3.0
  notes:     https://github.com/graphersal/graphersal-memory/releases/tag/v0.3.0

The upgrade, through the release's installer (it was installed with it):

  1. download https://github.com/graphersal/graphersal-memory/releases/download/v0.3.0/install.sh
  2. sh /tmp/graphersal-memory-upgrade-4242/install.sh --version v0.3.0 --dir /Users/you/.local/bin
  3. /Users/you/.local/bin/graphersal-memory daemon restart
  4. /Users/you/.local/bin/graphersal-memory install --refresh --yes (the installed parts to the new version)
  5. /Users/you/.local/bin/graphersal-memory --version

Upgrade now? [y/N]
```

How it upgrades follows where the binary is:

| Binary in | Upgrade through |
|---|---|
| `~/.local/bin` (Windows: `%LOCALAPPDATA%\Programs\graphersal-memory`) | the release's `install.sh` (Windows: `install.ps1` through PowerShell) of that version, into the same directory |
| `$CARGO_HOME/bin` or `~/.cargo/bin` | the release's installer with `--dir` set to cargo's directory, then cargo's records (`.crates.toml`, `.crates2.json`) moved to the new version, so `cargo install --list` and `cargo uninstall` stay right. `cargo install graphersal-memory --locked --force --version <latest>` builds it from source only when there is no release build for the platform, when the release's build did not install, or with `--from-source` (on Windows the running `.exe` is moved aside to `graphersal-memory.old.exe` first) |
| anywhere else | the release's installer with `--dir` set to that directory |

The installers verify the archive against the release's `SHA256SUMS` and replace a running binary
safely. A failed step leaves the installed version as it was.

Step 4 runs the new binary's `install --refresh`: what install wrote into the harnesses it is
installed in (the rule block, the MCP entries, the hooks, the rule file) is brought to the new
version. Every choice stays as it is: no harness is added, the harness's own memory, the develop
server and other memory tools stay on or off as they are, a memory tool found since the last
install is not named in the rule block, and nothing is imported, downloaded or written into
`config.json`. A part you edited by hand stays a conflict and is reported, as with `install`.
After an upgrade by hand (the installer or `cargo install`), run `graphersal-memory install
--refresh` yourself; `--dry-run` shows the plan first. `--check` only reports (with the
commands to run by hand), `--dry-run` stops after the plan, `--yes` does not ask (needed without
a terminal, also for a command run from an agent's prompt such as Claude Code's `!`),
`--from-source` builds a cargo install with cargo. `GRAPHERSAL_MEMORY_BASE_URL` points the check and the installer download at a mirror.

The browser UI shows "Version X available" in its top bar when a newer release is out; the
Overview then shows the command and the manual commands with a Copy button. It asks GitHub at
most once a day (`<data dir>/update-check.json` keeps the answer); `"updates": {"check": false}`
in `config.json` or `GRAPHERSAL_MEMORY_UPDATE_CHECK=off` switches the check off.

## What install and uninstall show

These are real outputs, captured on 2026-11-12 by running `graphersal-memory` (a release build
of the main branch shortly before 0.1.0) under a pseudo-terminal on throwaway home directories (`/tmp/gmw/h`
and the others named below, with the data and run directories chosen by `GRAPHERSAL_MEMORY_DIR` /
`GRAPHERSAL_MEMORY_RUN_DIR`, which is why the MCP entries carry `--data-dir` and `--run-dir`). The
agent programs were stand-ins on `PATH` (they are only looked for, never run, by install), so the
outputs show what install writes, not that a live agent accepted it. The binary's path is shown as
`/opt/gm/graphersal-memory`. Every wizard below chose the built-in model (`3`), so nothing was
downloaded; with the default model the report adds the download lines. Colour was off
(`NO_COLOR=1`); in a colour terminal the step titles are bold, the explanations dim, the default
green and ACTION NEEDED yellow. An answer of Enter shows as nothing after the prompt.

Paths under the home directory show as `~` in prose and file lists, never inside a command shown
between backticks (a shell does not expand `~` in quotes, so a copied command must carry the full
path). The full list of changes is behind `d` at the last step and in `install --dry-run`.

On Windows the same commands write the same parts into the same files under your profile
directory (`%USERPROFILE%\.claude.json`, `%USERPROFILE%\.claude\`, `%USERPROFILE%\.config\opencode\`,
`%USERPROFILE%\.codex\`, `%USERPROFILE%\.cursor\`), the data directory is
`%LOCALAPPDATA%\graphersal-memory`, and the hook commands are written so that they run without
Git Bash: Claude Code's step shows `` `"C:\...\graphersal-memory.exe" hook session-start` (run
without a shell) ``, and Codex's and Cursor's commands carry no quotes where they can
([Windows](harnesses.md#windows)). On macOS and Linux a hook command quotes its paths in single
quotes, so a `$`, a backtick or a backslash in a path stays literal. These outputs were not
captured on Windows; what is tested there and what is not yet is in
[Supported harnesses: Windows](harnesses.md#windows).

## Claude Code

### `graphersal-memory install`

The home held one project (`/tmp/gmw/work`, a git repository) with two Claude Code memory files
(`pnpm.md`, a feedback memory, and `release-day.md`, pinned in `MEMORY.md`) and a `settings.json`
of `{"theme": "dark"}`. Answers: Enter (English), `3` (built-in model), Enter (import), Enter
(switch Claude Code's auto-memory off), `2` (no summaries), `2` (no develop server), Enter
(install).

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
Choose [1]: 3

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
  Your rules and pinned memories still reach every session through graphersal-memory's
  start context: 2 memories, 387 characters (`graphersal-memory start-context` shows
  it).
  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new
  ones, is written back to Claude Code's memory files and its auto-memory is switched
  back on.

  1) Yes  switch it off (autoMemoryEnabled false in ~/.claude/settings.json)  (default)
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
  Embedding model: the built-in hash-ngram/1 (no download)
  File memory: 2 memory files imported now
  Auto-memory: switched off. Your rules and pinned memories still reach every session through graphersal-memory's start context: 2 memories, 387 characters (`graphersal-memory start-context` shows it).
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.

Install? [Y/n, d = show every change]: 

Report:
  Backups of the files changed: /tmp/gmw/d/install/backups/2026-11-11T202952Z/

Claude Code:
  created ~/.claude.json
  changed ~/.claude/settings.json (backed up)
  created ~/.claude/CLAUDE.md
  Claude Code's auto-memory: off (uninstall writes every memory back to its memory files and switches it on again)

Memory:
  created /tmp/gmw/d/config.json
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  imported 2 memories (2 new, 0 updated) into 1 project; 0 unchanged
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexing in the background (`graphersal-memory import --sessions` shows the progress)

----------------------------------------
Next steps
  1. Restart running Claude Code sessions to load the MCP server and hooks.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

The auto-memory question can switch Claude Code's own memory off without losing your rules:
Claude Code loads `MEMORY.md` into every session, and graphersal-memory's start context does the
same job from the graph. Every pinned memory and every feedback and user memory of the project
and the global ones go into the start of every session (memories under a `## Pinned` heading of
`MEMORY.md` are imported as pinned, and the import runs during install, so they are there from
the first session). The numbers in the step are what your memory files give (the project with the
most of them). When the start context is off (`memory.start_context_chars` is `0` in
`config.json`), the default answer is "No, keep it on" and the step says why. On a machine where
graphersal-memory is installed already, every default is the current state: a re-install keeps
the auto-memory value you set yourself after the first install.

### `graphersal-memory status` afterwards

(The start context line counts the memories of the project `status` runs in; it ran outside the
project here.)

```text
graphersal-memory status
  daemon: running (pid 67794, version 0.1.0, up 10s, 0 connection(s), stores open: default)
  data directory: /tmp/gmw/d
  run directory: /tmp/gmw/r
  settings: config.json (daemon idle time 7200s)
  sessions: 0 indexed, 0 summarized; summaries off
  store default: 56.8 KiB on disk (snapshots 8.1 KiB, WAL 47.6 KiB, models and other blobs 0 B); 1 snapshot(s)
    WAL since the last snapshot: 47.5 KiB (checkpoint after 16.0 MiB)
    last snapshot: 2026-11-11T20:29:52Z (commit 0); last prune: never
    history kept: 30 days

Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)
  start context: 0 memories, 0 of 8000 characters

Claude Code:
  user configuration: ~/.claude.json
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: not registered (off; `graphersal-memory install --enable-develop` turns it on)
  hook SessionStart (~/.claude/settings.json): installed
  hook SessionEnd (~/.claude/settings.json): installed
  rule block (~/.claude/CLAUDE.md): installed
  file memory: 2 memory files imported so far
  auto-memory (~/.claude/settings.json): off (autoMemoryEnabled: false, set by install; uninstall exports every memory back and switches it on again)
```

### Other memory on the machine

When another memory is found (see [harnesses.md](harnesses.md#other-memory-beside-graphersal-memory)),
the wizard gets a step "Other memory", the plan a section of the same name, and `status` and
`doctor` an "Other memory" section. A machine with Claude Code's auto-memory on, the reference
memory server registered as `memory`, and a `CLAUDE.md` line saying to keep memory there:

```
Step 5 of 8 · Other memory
  graphersal-memory found other memory on this machine. Two memories split the facts:
  an agent saves into one and later searches the other.
  - memory (Claude Code, ~/.claude.json): the reference memory server
  (@modelcontextprotocol/server-memory)
  - ~/.claude/CLAUDE.md:4 says "- Keep all memory in the knowledge graph of the memory
  MCP server (create_entities)." (never edited; `graphersal-memory doctor` suggests an
  edit)

  memory:
  1) graphersal-memory is the main memory  memory stays, named read-only in the rule block  (default)
                                           (search it when needed, never write there)
  2) Switch memory off                     in ~/.claude.json; backed up, uninstall switches it on again
  3) Keep both as they are                 the rule block does not name it
Choose [1]:
  Import:
  1) Yes  import 2 memories of memory now (global memories; the files are only read)  (default)
  2) No   not now (`graphersal-memory install` offers it again)
```

The plan (`--dry-run`, or `d` in the last step):

```
Other memory
  8. Other memory tools
     - memory (Claude Code, ~/.claude.json): the reference memory server (@modelcontextprotocol/server-memory); stays, named read-only in the rule block (graphersal-memory is the main memory)
  9. Import the other memory tools' memories
     - memory (the reference memory server (@modelcontextprotocol/server-memory)): 2 memories in ~/kg/memory.jsonl, 2 new or changed: imported as global memories (the file is only read)
  10. Instruction files that mention other memory (never changed by graphersal-memory)
     - ~/.claude/CLAUDE.md:4: "- Keep all memory in the knowledge graph of the memory MCP server (create_entities)." (mentions other memory: check it; never changed, `graphersal-memory doctor` suggests an edit)
```

`status` and `doctor` afterwards:

```
Other memory:
  Claude Code's auto-memory: off; 2 memory files in 1 project (~/.claude/projects)
  memory (Claude Code, ~/.claude.json): the reference memory server (@modelcontextprotocol/server-memory); named read-only in the rule block (graphersal-memory is the main memory)
  ~/.claude/CLAUDE.md:4 says: "- Keep all memory in the knowledge graph of the memory MCP server (create_entities)." (never edited; `graphersal-memory doctor` suggests an edit)
```

```
Other memory:
  ok      Claude Code's auto-memory: off; 2 memory files in 1 project (~/.claude/projects)
  ok      memory (Claude Code, ~/.claude.json): the reference memory server (@modelcontextprotocol/server-memory); named read-only in the rule block (graphersal-memory is the main memory)
  note    ~/.claude/CLAUDE.md:4 says: "- Keep all memory in the knowledge graph of the memory MCP server (create_entities)."
          if it tells the agent to keep memory there, it disagrees with graphersal-memory's rule block (the agent is told to point that out). graphersal-memory never edits your instruction files: delete the line, or change it to "Memory lives in graphersal-memory (the MCP server `graphersal-memory`)."
```

These are notes, never problems or ACTION NEEDED lines: nothing here breaks graphersal-memory,
and what to do is your choice. The flags: `--other-read-only NAME`, `--keep-other NAME`,
`--disable-other NAME` (the only way to switch another tool off without the wizard; `--yes` alone
never does), `--enable-other NAME`. `graphersal-memory import --ignore PATH` stops importing a
memory file or directory another program keeps writing (`--unignore PATH` undoes it).

### `graphersal-memory uninstall`

Answers: Enter (export), Enter (uninstall). Both memory files said the same as the graph, so the
export left them (and `MEMORY.md`) untouched; a memory changed in the graph since install would
have been written into its file when that file was still the one the import read, or beside it as
`<slug>.graphersal.md` when you had edited it since. `settings.json` got its original bytes back
(`{"theme": "dark"}`, auto-memory on again).

```text
graphersal-memory uninstall (Claude Code)

First, the memories:
  - export 2 memories of 1 project (0 global) back to Claude Code's file memory (MEMORY.md and one file per memory; the global ones into ~/.claude/rules/graphersal-memory-global.md); a file that differs is kept

Claude Code:
  - ~/.claude.json: delete the file (install created it)
  - ~/.claude/settings.json: restore the file as it was before install
  - ~/.claude/CLAUDE.md: delete the file (install created it)

Export 2 memories of 1 project back to Claude Code's file memory? [Y/n] 

Uninstall? [Y/n] 

Report:
  exported 2 memories of 1 project (plus the global ones): 0 files written, 3 unchanged

Claude Code:
  Claude Code's auto-memory: back to how it was before install
  removed ~/.claude.json
  changed ~/.claude/settings.json
  removed ~/.claude/CLAUDE.md

Your memories stay in /tmp/gmw/d (`uninstall --purge` deletes them).
```

## opencode

A throwaway home (`/tmp/gmo/h`) holding only an empty `~/.config/opencode/` directory, so opencode
was the one harness found and the wizard did not ask which agents; `opencode --version` answered
2.2.4. Answers: Enter (English), `3` (built-in model), `2` (no summaries), `2` (no develop
server), Enter (install). There is no auto-memory question: opencode has no memory feature of its
own. The summaries step names opencode's own headless command with a free model, so its estimate
is $0.00. With opencode 1.x (`--version` 1.19.5, same answers) the wizard and the report are the
same; the configuration then gets 1.x's shape (members of `mcp` with `"enabled"`).

```text
graphersal-memory install (opencode)
A few questions; Enter takes the default. Nothing changes before the last step.

Step 1 of 5 · Memory language
  Memories are found by meaning. Keeping them in one language lets a search find a
  memory whatever language you wrote in; or keep each memory in your own words.

  1) English only  the agent stores and searches in English  (default)
                   (it translates a query before searching)
  2) Any language  the agent stores in the language of the conversation
                   (and searches in it and in English)
Choose [1]: 

Step 2 of 5 · Embedding model
  The model turns every memory into a vector, so a search finds related wording,
  not only the same words. Downloaded once from Hugging Face; it then runs locally.

  1) potion-retrieval-32M      English, ~130 MB, MIT  (default)
  2) potion-multilingual-128M  100+ languages, ~530 MB, MIT
  3) Built-in (no download)    works offline, but finds only shared words and spellings,
                               not meaning; weaker search
Choose [1]: 3

Step 3 of 5 · Session summaries
  Past sessions are indexed locally (their raw turns, no model call). Optionally each
  finished session is summarized through your own `opencode run --model
  opencode/big-pickle`
  (decisions, corrections, outcomes, open threads). With this free model:
  0 finished sessions, 0 without a summary: $0.00 (`opencode run --model
  opencode/big-pickle` uses a free model of opencode Zen, free for a limited time; the
  monthly budget still applies)

  1) Yes  summarize, at most $5.00 per month
  2) No   raw turns only (`graphersal-memory summaries enable` later)
Choose 1-2: 2

Step 4 of 5 · The develop server
  A second MCP server with tools to inspect and reshape the memory's structure
  (schema, saved queries, indexes). Most people do not need it; its tool
  descriptions are paid on every agent turn.

  1) Yes  register graphersal-memory-develop
  2) No   not now (`graphersal-memory install --enable-develop` later)
Choose 1-2: 2

Step 5 of 5 · Ready to install

  Agents: opencode
  Memory language: English only (stored and searched in English)
  Embedding model: the built-in hash-ngram/1 (no download)
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

Install? [Y/n, d = show every change]: 

Report:

opencode:
  created ~/.config/opencode/opencode.json
  created ~/.config/opencode/graphersal-memory.md
  created ~/.config/opencode/plugins/graphersal-memory.js

Memory:
  created /tmp/gmo/d/config.json
  summaries run through `opencode run --model opencode/big-pickle` when you switch them on (opencode only; a free model of opencode Zen; summaries.harness_command in config.json)
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

----------------------------------------
Next steps
  1. Restart running opencode sessions to load the MCP server and plugin.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

The configuration file it created (2.x's shape: `mcp.servers`, `"disabled"`, `"codemode": false`
so the tools stay direct calls):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "graphersal-memory": {
        "codemode": false,
        "command": [
          "/opt/gm/graphersal-memory",
          "mcp",
          "--profile",
          "use",
          "--data-dir",
          "/tmp/gmo/d",
          "--run-dir",
          "/tmp/gmo/r"
        ],
        "disabled": false,
        "type": "local"
      },
      "graphersal-memory-develop": {
        "codemode": false,
        "command": [
          "/opt/gm/graphersal-memory",
          "mcp",
          "--profile",
          "develop",
          "--data-dir",
          "/tmp/gmo/d",
          "--run-dir",
          "/tmp/gmo/r"
        ],
        "disabled": true,
        "type": "local"
      }
    }
  },
  "instructions": [
    "/tmp/gmo/h/.config/opencode/graphersal-memory.md"
  ]
}
```

### `graphersal-memory status` afterwards

```text
graphersal-memory status
  daemon: not running (the next agent session starts it)
  data directory: /tmp/gmo/d
  run directory: /tmp/gmo/r
  settings: config.json (daemon idle time 7200s)
  sessions: 0 indexed, 0 summarized; summaries off

Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)

opencode:
  configuration: ~/.config/opencode/opencode.json
  version: opencode 2.x (2.2.4, from `/tmp/gmo/f/opencode --version`)
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: registered, disabled (`graphersal-memory install --harness opencode --enable-develop` enables it)
  rules file in instructions: installed
  rule block (~/.config/opencode/graphersal-memory.md): installed
  session plugin (~/.config/opencode/plugins/graphersal-memory.js): installed
  auto-memory: opencode has none (nothing to switch off)
  headless command (summaries, bench): `opencode run --model opencode/big-pickle` by default
  sessions: read from ~/.local/share/opencode/opencode.db (not there yet)
```

### `graphersal-memory uninstall`

The store held no memories, so the export wrote nothing; with memories it writes readable
Markdown under `~/graphersal-memory-export/opencode/` (per project `MEMORY.md` and one file per
memory, `global.md` for the global ones) and says how to give opencode a file of it
(`instructions`). Every file install created is deleted, and the `plugins/` directory it made with
it.

```text
graphersal-memory uninstall (opencode)

First, the memories:
  - export 0 memories of 0 projects (0 global) into readable Markdown files under ~/graphersal-memory-export/opencode (opencode has no file memory; nothing is deleted)

opencode:
  - ~/.config/opencode/opencode.json: delete the file (install created it)
  - ~/.config/opencode/graphersal-memory.md: delete the file (install created it)
  - ~/.config/opencode/plugins/graphersal-memory.js: delete the file (install created it)

Uninstall? [Y/n] 

Report:
  exported nothing: the graph holds no memories

opencode:
  removed ~/.config/opencode/opencode.json
  removed ~/.config/opencode/graphersal-memory.md
  removed ~/.config/opencode/plugins/graphersal-memory.js
  removed the directory ~/.config/opencode/plugins

Your memories stay in /tmp/gmo/d (`uninstall --purge` deletes them).
```

### The smoke run with a real opencode 2.x (2026-11-10)

On a throwaway home with opencode 2.2.4: `opencode run --model opencode/big-pickle` was asked to
save "We release on Fridays" with the remember tool (it called `graphersal-memory_memories`, then
`graphersal-memory_remember`, as direct tools), and a second session was asked on which weekday
we release (it called `graphersal-memory_memories` and answered "Friday."). The plugin's start
hook queued both sessions as running; `opencode service stop` (2.x's background server) ran the
plugin's cleanup, whose end hook got both indexed (`2 found, 2 indexed`). `uninstall --yes`
exported the memory to `~/graphersal-memory-export/opencode/global.md` and deleted the three files
install had created. A summary through `opencode run --standalone` ran in its sandbox and came
back with opencode Zen's "Free usage limit reached" (the free model's daily limit was used up by
the smoke run), reported as a failed summary. opencode 2.x still loads MCP servers of 1.x's shape
written directly under `mcp` beside `mcp.servers` (its `normalizeMcp` merges both, checked in its
v2.2.4 source), so a server of yours in the old shape keeps working after install.

## Claude Code and opencode together

A home with both `~/.claude/` and `~/.config/opencode/` (`/tmp/gmb/h`). The wizard starts with
"Which agents"; Enter keeps both. Both get the same MCP server command and data directory: one
store, one daemon, the same memory of a project from either agent. Answers: Enter (both), Enter
(English), `3` (built-in), Enter (switch Claude Code's auto-memory off), `2`, `2`, Enter.

```text
graphersal-memory install (Claude Code and opencode)
A few questions; Enter takes the default. Nothing changes before the last step.

Step 1 of 7 · Which agents
  graphersal-memory found these agent harnesses on this machine. Every one you
  choose gets the same memory: one store and one background daemon, so a project's
  memory is the same from each agent.

  1) All of them       Claude Code and opencode  (default)
  2) Claude Code only
  3) opencode only
Choose [1]: 

Step 2 of 7 · Memory language
  Memories are found by meaning. Keeping them in one language lets a search find a
  memory whatever language you wrote in; or keep each memory in your own words.

  1) English only  the agent stores and searches in English  (default)
                   (it translates a query before searching)
  2) Any language  the agent stores in the language of the conversation
                   (and searches in it and in English)
Choose [1]: 

Step 3 of 7 · Embedding model
  The model turns every memory into a vector, so a search finds related wording,
  not only the same words. Downloaded once from Hugging Face; it then runs locally.

  1) potion-retrieval-32M      English, ~130 MB, MIT  (default)
  2) potion-multilingual-128M  100+ languages, ~530 MB, MIT
  3) Built-in (no download)    works offline, but finds only shared words and spellings,
                               not meaning; weaker search
Choose [1]: 3

Step 4 of 7 · Claude Code's auto-memory
  Claude Code also writes its own memory files and loads them into every session;
  with graphersal-memory both would keep the same facts twice.
  Your rules and pinned memories reach every session through graphersal-memory's start
  context (none in your memory files yet; `graphersal-memory start-context` shows it).
  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new
  ones, is written back to Claude Code's memory files and its auto-memory is switched
  back on.

  1) Yes  switch it off (autoMemoryEnabled false in ~/.claude/settings.json)  (default)
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

  Agents: Claude Code and opencode
  Memory language: English only (stored and searched in English)
  Embedding model: the built-in hash-ngram/1 (no download)
  File memory: nothing new to import
  Auto-memory: switched off. Your rules and pinned memories reach every session through graphersal-memory's start context (none in your memory files yet; `graphersal-memory start-context` shows it).
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.

Install? [Y/n, d = show every change]: 

Report:

Claude Code:
  created ~/.claude.json
  created ~/.claude/settings.json
  created ~/.claude/CLAUDE.md
  Claude Code's auto-memory: off (uninstall writes every memory back to its memory files and switches it on again)

opencode:
  created ~/.config/opencode/opencode.json
  created ~/.config/opencode/graphersal-memory.md
  created ~/.config/opencode/plugins/graphersal-memory.js

Memory:
  created /tmp/gmb/d/config.json
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  imported nothing: no new or changed memory files
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

----------------------------------------
Next steps
  1. Restart running Claude Code and opencode sessions to load the MCP server, hooks and plugin.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

## Codex

A throwaway home (`/tmp/gmc/h`) holding only `~/.codex/config.toml` (`model = "gpt-5.5"` and
`[features] memories = true`) and a one-line `~/.codex/AGENTS.md`, so Codex was the one harness
found. Because Codex's own memories were on, the wizard asked about them (with Codex's memories
off, which is Codex's default, that step is not shown). Answers: Enter (English), `3` (built-in
model), Enter (switch Codex's memories off), `2` (no summaries), `2` (no develop server), Enter
(install). Codex runs a hook only after you trust it in Codex itself, so the last step, the report
and `status` say so as ACTION NEEDED.

```text
graphersal-memory install (Codex)
A few questions; Enter takes the default. Nothing changes before the last step.

Step 1 of 6 · Memory language
  Memories are found by meaning. Keeping them in one language lets a search find a
  memory whatever language you wrote in; or keep each memory in your own words.

  1) English only  the agent stores and searches in English  (default)
                   (it translates a query before searching)
  2) Any language  the agent stores in the language of the conversation
                   (and searches in it and in English)
Choose [1]: 

Step 2 of 6 · Embedding model
  The model turns every memory into a vector, so a search finds related wording,
  not only the same words. Downloaded once from Hugging Face; it then runs locally.

  1) potion-retrieval-32M      English, ~130 MB, MIT  (default)
  2) potion-multilingual-128M  100+ languages, ~530 MB, MIT
  3) Built-in (no download)    works offline, but finds only shared words and spellings,
                               not meaning; weaker search
Choose [1]: 3

Step 3 of 6 · Codex's memories
  Codex's own memory feature is on: it generates memories from your chats and
  loads them into sessions; with graphersal-memory both would keep the same facts
  twice.
  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new
  ones, is exported as readable Markdown files and Codex's memories are switched back
  on.

  1) Yes  switch it off ([features] memories = false in ~/.codex/config.toml)  (default)
  2) No   keep it on
Choose [1]: 

Step 4 of 6 · Session summaries
  Past sessions are indexed locally (their raw turns, no model call). Optionally each
  finished session is summarized through your own `codex exec`
  (decisions, corrections, outcomes, open threads). That costs model usage:
  0 finished sessions, 0 without a summary: about $0.00 at list price (an estimate: ~4
  characters per token, $3/$15 per million input/output tokens, plus the harness's own
  prompt; your harness reports the real cost)

  1) Yes  summarize, at most $5.00 per month
  2) No   raw turns only (`graphersal-memory summaries enable` later)
Choose 1-2: 2

Step 5 of 6 · The develop server
  A second MCP server with tools to inspect and reshape the memory's structure
  (schema, saved queries, indexes). Most people do not need it; its tool
  descriptions are paid on every agent turn.

  1) Yes  register graphersal-memory-develop
  2) No   not now (`graphersal-memory install --enable-develop` later)
Choose 1-2: 2

Step 6 of 6 · Ready to install

  Agents: Codex
  Memory language: English only (stored and searched in English)
  Embedding model: the built-in hash-ngram/1 (no download)
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is exported as readable Markdown files and Codex's memories are switched back on.
  ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.

Install? [Y/n, d = show every change]: 

Report:
  Backups of the files changed: /tmp/gmc/d/install/backups/2026-11-11T203141Z/

Codex:
  changed ~/.codex/config.toml (backed up)
  created ~/.codex/hooks.json
  changed ~/.codex/AGENTS.md (backed up)
  Codex's memories: off (uninstall exports every memory as Markdown and switches them on again)

Memory:
  created /tmp/gmc/d/config.json
  summaries run through `codex exec` when you switch them on (Codex only; the model of your Codex configuration; summaries.harness_command in config.json)
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

----------------------------------------
Next steps
  1. Restart running Codex sessions to load the MCP server and hooks.
  2. ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

`~/.codex/config.toml` afterwards (the block went before the first table, `[features]`; the
memories value was changed in place; `--data-dir` and `--run-dir` appear because this run chose
its directories explicitly, with the defaults the arguments are just `["mcp", "--profile",
"use"]`):

```toml
model = "gpt-5.5"

# graphersal-memory:begin fingerprint=ef92e80927631e1e (managed by `graphersal-memory install`; `graphersal-memory uninstall` removes it; an edit inside is reported as a conflict)
[mcp_servers.graphersal-memory]
command = "/opt/gm/graphersal-memory"
args = ["mcp", "--profile", "use", "--data-dir", "/tmp/gmc/d", "--run-dir", "/tmp/gmc/r"]
startup_timeout_sec = 30

[mcp_servers.graphersal-memory-develop]
command = "/opt/gm/graphersal-memory"
args = ["mcp", "--profile", "develop", "--data-dir", "/tmp/gmc/d", "--run-dir", "/tmp/gmc/r"]
startup_timeout_sec = 30
enabled = false
# graphersal-memory:end

[features]
memories = false
```

### `graphersal-memory status` afterwards

```text
graphersal-memory status
  daemon: not running (the next agent session starts it)
  data directory: /tmp/gmc/d
  run directory: /tmp/gmc/r
  settings: config.json (daemon idle time 7200s)
  sessions: 0 indexed, 0 summarized; summaries off

Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)

Codex:
  configuration: ~/.codex/config.toml
  program: /tmp/gmc/f/codex (2.1.0)
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: registered, disabled (`graphersal-memory install --harness codex --enable-develop` enables it)
  hook SessionStart (~/.codex/hooks.json): installed
  hook SessionEnd (~/.codex/hooks.json): installed
  hook trust: NOT APPROVED YET
  ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
  rule block (~/.codex/AGENTS.md): installed
  Codex memories: off (switched off by install; uninstall switches them on again)
  headless command (summaries, bench): `codex exec` by default
  sessions: read from ~/.codex/sessions (not there yet)

ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
```

### `graphersal-memory uninstall`

Answer: Enter (uninstall; the store held no memories, so there was no export question).
Afterwards `~/.codex/` held its files as before the install.

```text
graphersal-memory uninstall (Codex)

First, the memories:
  - export 0 memories of 0 projects (0 global) into readable Markdown files under ~/graphersal-memory-export/codex (Codex has no file memory; nothing is deleted)

Codex:
  - ~/.codex/config.toml: restore the file as it was before install
  - ~/.codex/hooks.json: delete the file (install created it)
  - ~/.codex/AGENTS.md: restore the file as it was before install

Uninstall? [Y/n] 

Report:
  exported nothing: the graph holds no memories

Codex:
  Codex's memories: switched on again, as before install
  changed ~/.codex/config.toml
  removed ~/.codex/hooks.json
  changed ~/.codex/AGENTS.md

Your memories stay in /tmp/gmc/d (`uninstall --purge` deletes them).
```

## Cursor

Cursor is not run here (it needs a login). A home with `~/.claude` and `~/.cursor` (`/tmp/gmu/h`),
so both harnesses were found and the wizard asked which agents. Answers: `3` (Cursor only), Enter
(English), `3` (built-in model), `2` (no summaries), `2` (no develop server), Enter (install). The
step count drops after the choice: the Claude Code steps do not apply to Cursor.

```text
graphersal-memory install (Claude Code and Cursor)
A few questions; Enter takes the default. Nothing changes before the last step.

Step 1 of 7 · Which agents
  graphersal-memory found these agent harnesses on this machine. Every one you
  choose gets the same memory: one store and one background daemon, so a project's
  memory is the same from each agent.

  1) All of them       Claude Code and Cursor  (default)
  2) Claude Code only
  3) Cursor only
Choose [1]: 3

Step 2 of 6 · Memory language
  Memories are found by meaning. Keeping them in one language lets a search find a
  memory whatever language you wrote in; or keep each memory in your own words.

  1) English only  the agent stores and searches in English  (default)
                   (it translates a query before searching)
  2) Any language  the agent stores in the language of the conversation
                   (and searches in it and in English)
Choose [1]: 

Step 3 of 6 · Embedding model
  The model turns every memory into a vector, so a search finds related wording,
  not only the same words. Downloaded once from Hugging Face; it then runs locally.

  1) potion-retrieval-32M      English, ~130 MB, MIT  (default)
  2) potion-multilingual-128M  100+ languages, ~530 MB, MIT
  3) Built-in (no download)    works offline, but finds only shared words and spellings,
                               not meaning; weaker search
Choose [1]: 3

Step 4 of 6 · Session summaries
  Past sessions are indexed locally (their raw turns, no model call). Optionally each
  finished session is summarized through your own `claude -p --model sonnet`
  (decisions, corrections, outcomes, open threads). That costs model usage:
  0 finished sessions, 0 without a summary: about $0.00 at list price (an estimate: ~4
  characters per token, $3/$15 per million input/output tokens, plus the harness's own
  prompt; your harness reports the real cost)
  Cursor's own CLI is not used for summaries (it reports no cost); say yes only if
  the command above is installed and logged in.

  1) Yes  summarize, at most $5.00 per month
  2) No   raw turns only (`graphersal-memory summaries enable` later)
Choose 1-2: 2

Step 5 of 6 · The develop server
  A second MCP server with tools to inspect and reshape the memory's structure
  (schema, saved queries, indexes). Most people do not need it; its tool
  descriptions are paid on every agent turn.

  1) Yes  register graphersal-memory-develop
  2) No   not now (`graphersal-memory install --enable-develop` later)
Choose 1-2: 2

Step 6 of 6 · Ready to install

  Agents: Cursor
  Memory language: English only (stored and searched in English)
  Embedding model: the built-in hash-ngram/1 (no download)
  Past sessions: 0 to index locally in the background (raw turns)
  Summaries: off

Install? [Y/n, d = show every change]: 

Report:

Cursor:
  created ~/.cursor/mcp.json
  created ~/.cursor/hooks.json
  created ~/.cursor/rules/graphersal-memory.mdc

Memory:
  created /tmp/gmu/d/config.json
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

----------------------------------------
Next steps
  1. Restart running Cursor sessions to load the MCP server and hooks.
  `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

### `graphersal-memory status` afterwards

```text
graphersal-memory status
  daemon: not running (the next agent session starts it)
  data directory: /tmp/gmu/d
  run directory: /tmp/gmu/r
  settings: config.json (daemon idle time 7200s)
  sessions: 0 indexed, 0 summarized; summaries off

Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)

Cursor:
  configuration: ~/.cursor
  program: /tmp/gmu/f/cursor
  MCP server graphersal-memory (use, ~/.cursor/mcp.json): installed
  MCP server graphersal-memory-develop: not registered (`graphersal-memory install --harness cursor --enable-develop` registers it)
  hook sessionStart (~/.cursor/hooks.json): installed
  hook sessionEnd (~/.cursor/hooks.json): installed
  rule file (~/.cursor/rules/graphersal-memory.mdc): installed
  Cursor memories: none in a file (nothing to switch off)
  headless command (summaries, bench): none (Cursor's CLI is not used; summaries run through Claude Code, opencode or Codex when one is installed)
  sessions: 0 transcript(s) reported by the hooks (0 on disk), registry /tmp/gmu/d/sessions/cursor-transcripts.json

Other agents:
  Claude Code: found, not set up (`graphersal-memory install --harness claude-code`)
```

### `graphersal-memory uninstall --harness cursor`

```text
graphersal-memory uninstall (Cursor)

First, the memories:
  - export 0 memories of 0 projects (0 global) into readable Markdown files under ~/graphersal-memory-export/cursor (Cursor has no file memory; nothing is deleted)

Cursor:
  - ~/.cursor/mcp.json: delete the file (install created it)
  - ~/.cursor/hooks.json: delete the file (install created it)
  - ~/.cursor/rules/graphersal-memory.mdc: delete the file (install created it)

Uninstall? [Y/n] 

Report:
  exported nothing: the graph holds no memories

Cursor:
  removed ~/.cursor/mcp.json
  removed ~/.cursor/hooks.json
  removed ~/.cursor/rules/graphersal-memory.mdc
  removed the directory ~/.cursor/rules

Your memories stay in /tmp/gmu/d (`uninstall --purge` deletes them).
```

A live run with Cursor is still to be done: [cursor-live-check.md](cursor-live-check.md).
