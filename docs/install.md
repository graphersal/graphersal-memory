# Install and uninstall: what you see

These are real outputs, captured on 2026-11-10 by running `graphersal-memory` (a development
build) under a pseudo-terminal on a throwaway home directory (`/tmp/gmw/h`, with the data and run
directories chosen by `GRAPHERSAL_MEMORY_DIR` / `GRAPHERSAL_MEMORY_RUN_DIR`, which is why the hook
commands carry `--data-dir` and `--run-dir`). The home held one project (`/tmp/gmw/work`, a git
repository) with two Claude Code memory files and a `settings.json` of `{"theme": "dark"}`.
Colour was off (`NO_COLOR=1`); in a colour terminal the step titles are bold, the explanations
dim and the default green. An answer of Enter shows as nothing after the prompt.

On Windows the same commands write the same parts into the same files under your profile
directory (`%USERPROFILE%\.claude.json`, `%USERPROFILE%\.claude\`, `%USERPROFILE%\.config\opencode\`,
`%USERPROFILE%\.codex\`, `%USERPROFILE%\.cursor\`), the data directory is
`%LOCALAPPDATA%\graphersal-memory`, and the hook commands are written so that they run without
Git Bash: Claude Code's step shows `` `"C:\...\graphersal-memory.exe" hook session-start` (run
without a shell) ``, and Codex's and Cursor's commands carry no quotes where they can
([Windows](harnesses.md#windows)). These
outputs were not captured on Windows; what is tested there and what is not yet is in
[Supported harnesses: Windows](harnesses.md#windows).

**Layout since M13 (2026-11-11).** The transcripts below were captured before the output was
regrouped; the content of each line is the same, the layout differs: the wizard's last step is
now a short recap (agents, language, model, file memory, past sessions, summaries, plus any
conflict, Codex's ACTION NEEDED and the "Nothing is lost" line) ending in `Install? [Y/n, d =
show every change]`, and the full plan appears behind `d` and with `--dry-run`, in sections
(shared, one per agent, then import and indexing); the report names the backup directory once,
groups its lines per agent and ends with a numbered "Next steps" block after a separator; paths
under the home directory show as `~`; `status` and `doctor` set each section off by a blank
line (`status` puts Memory before the agents and repeats ACTION NEEDED as its last line);
uninstall asks `Uninstall? [Y/n]` and, with nothing to answer (a pipe), says that it needs a
terminal or `--yes`. A harness is offered only when its program is found (see the README).

## `graphersal-memory install`

Answers: Enter, Enter, Enter, Enter, `2`, `2`, Enter. The model download is real (Hugging Face);
the daemon then re-embedded the memory with it in the background (its log: "the store default
uses the embedding model minishlab/potion-retrieval-32M now (re-embedded)").

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
  Your rules and pinned memories still reach every session through graphersal-memory's
  start context: N memories, X characters (`graphersal-memory start-context` shows it).
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

Step 7 of 7 · Summary
  This is what install will do:

  1. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  2. Embedding model
     - download minishlab/potion-retrieval-32M (~130 MB, MIT) from https://huggingface.co into /tmp/gmw/d/models (the one network access of install; if it fails, the built-in embedder stays and install goes on)
     - the daemon imports it into the store and re-embeds every memory, session, turn and fact with it in the background (calls wait while it runs)
  3. Register the MCP servers at user scope in /tmp/gmw/h/.claude.json (key mcpServers)
     - graphersal-memory (use profile): add
     - graphersal-memory-develop: not registered (off; `graphersal-memory install --enable-develop` registers it)
  4. Add the SessionStart and SessionEnd hooks in /tmp/gmw/h/.claude/settings.json
     - SessionStart: add `"/private/tmp/gmw/bin/graphersal-memory" hook session-start --data-dir "/tmp/gmw/d" --run-dir "/tmp/gmw/r"`
     - SessionEnd: add `"/private/tmp/gmw/bin/graphersal-memory" hook session-end --data-dir "/tmp/gmw/d" --run-dir "/tmp/gmw/r"`
     - autoMemoryEnabled: false (switch Claude Code's auto-memory off). Your rules and pinned memories still reach every session through graphersal-memory's start context: N memories, X characters (`graphersal-memory start-context` shows it). Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.
  5. Import the existing file memory (MEMORY.md and the memory files)
     - 1 memory directory under /tmp/gmw/h/.claude/projects: 2 new and 0 changed memory files to import, 0 unchanged
     - The files are read, not changed; every memory stays in its project. CLAUDE.md rules are not imported in this version.
  6. Write the managed rule block into /tmp/gmw/h/.claude/CLAUDE.md
     - create the file with the block (your own text stays as it is)
  7. Index past sessions (raw turns, locally, in the background)
     - 1 session transcript under /tmp/gmw/h/.claude/projects: 0 to index, 1 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  8. Settings file /tmp/gmw/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.

Apply these changes? [Enter = apply, n = cancel]: 

Embedding model:
  Downloading minishlab/potion-retrieval-32M from https://huggingface.co
    config.json: 202 B / 202 B (100%)
    tokenizer.json: 1.4 MiB / 1.4 MiB (100%)
    model.safetensors: 123.2 MiB / 123.2 MiB (100%)
    README.md: 16.7 KiB / 16.7 KiB (100%)

Report:
  created /tmp/gmw/h/.claude.json
  changed /tmp/gmw/h/.claude/settings.json (backup: /tmp/gmw/d/install/backups/2026-11-10T072304Z/settings.json)
  created /tmp/gmw/h/.claude/CLAUDE.md
  Claude Code's auto-memory: off (uninstall writes every memory back to its memory files and switches it on again)
  created /tmp/gmw/d/config.json
  embedding model: downloaded minishlab/potion-retrieval-32M (124.6 MiB, licence mit, model2vec/minishlab/potion-retrieval-32M@c7f8727ce247) into /tmp/gmw/d/models/minishlab/potion-retrieval-32M
  embedding model: minishlab/potion-retrieval-32M (the daemon re-embeds the memory with it in the background)
  imported 2 memories (2 new, 0 updated) into 1 project; 0 unchanged
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexing in the background (`graphersal-memory import --sessions` shows the progress)

Restart running Claude Code sessions to load the MCP server and the hooks. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

The auto-memory question can switch Claude Code's own memory off without losing your rules:
Claude Code loads `MEMORY.md` into every session, and graphersal-memory's start context does the
same job from the graph. Every pinned memory and every feedback and user memory of the project
and the global ones go into the start of every session (memories under a `## Pinned` heading of
`MEMORY.md` are imported as pinned, and the import runs during install, so they are there from
the first session). N and X are what your memory files give (the project with the most of them).
When the start context is off (`memory.start_context_chars` is `0` in `config.json`), the default
answer is "No, keep it on" and the step says why.


## `graphersal-memory status` afterwards

```text
daemon: running (pid 53793, version 0.1.0, up 27s, 0 connection(s), stores open: default, run directory /tmp/gmw/r)
data directory: /tmp/gmw/d
settings: /tmp/gmw/d/config.json (daemon idle time 7200s)
sessions: 0 indexed, 0 summarized; summaries off
Claude Code (user configuration /tmp/gmw/h/.claude.json):
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: not registered (off; `graphersal-memory install --enable-develop` turns it on)
  hook SessionStart (/tmp/gmw/h/.claude/settings.json): installed
  hook SessionEnd (/tmp/gmw/h/.claude/settings.json): installed
  rule block (/tmp/gmw/h/.claude/CLAUDE.md): installed
  file memory: 2 memory files imported so far
  auto-memory (/tmp/gmw/h/.claude/settings.json): off (autoMemoryEnabled: false, set by install; uninstall exports every memory back and switches it on again)
  memory language: English only (stored and searched in English)
  embedding model: minishlab/potion-retrieval-32M (124.6 MiB, mit, downloaded)
```

## `graphersal-memory uninstall`

After one more memory was saved through MCP (`staging-first`). Answers: Enter (export), `y`.
`pnpm.md` and `release-day.md` were already identical to the graph's version and stayed
untouched; the new memory got its file; `MEMORY.md` now differs (it lacks the new memory), so it
was kept and the graph's index written beside it. (Since stage M7a this is different: a
`MEMORY.md` that differs keeps every line and gets only the lines of the memories it does not
list yet appended, here `- [...](staging-first.md) — ...`; the report says "appended 1 index line
to .../MEMORY.md". The transcript above predates that change.) `settings.json` got its original bytes back
(`{"theme": "dark"}`, auto-memory on again).

```text
graphersal-memory uninstall (Claude Code)

  - export 3 memories of 1 project (0 global) back to Claude Code's file memory (MEMORY.md and one file per memory; the global ones into /tmp/gmw/h/.claude/rules/graphersal-memory-global.md); a file that differs is kept
  - /tmp/gmw/h/.claude.json: delete the file (install created it)
  - /tmp/gmw/h/.claude/settings.json: restore the file as it was before install
  - /tmp/gmw/h/.claude/CLAUDE.md: delete the file (install created it)

Export 3 memories of 1 project back to Claude Code's file memory? [Y/n] 

Proceed? [y/N] y

Report:
  exported 3 memories of 1 project (plus the global ones): 1 file written, 2 unchanged
    wrote /tmp/gmw/h/.claude/projects/-private-tmp-gmw-work/memory/staging-first.md
    kept /tmp/gmw/h/.claude/projects/-private-tmp-gmw-work/memory/MEMORY.md (it differs from the graph's version); the graph's version is in /tmp/gmw/h/.claude/projects/-private-tmp-gmw-work/memory/MEMORY.graphersal.md
  Claude Code's auto-memory: back to how it was before install
  removed /tmp/gmw/h/.claude.json
  changed /tmp/gmw/h/.claude/settings.json
  removed /tmp/gmw/h/.claude/CLAUDE.md
  Your memories stay in /tmp/gmw/d (`uninstall --purge` deletes them).
```

## opencode

### opencode 1.x

Captured on 2026-11-10 like the outputs above, on a throwaway home (`/tmp/gmo/h`, shown with that
path) holding only an empty `~/.config/opencode/` directory, so opencode was the one harness found
and the wizard did not ask which agents. Answers: Enter (English), `3` (built-in model, no
download), `2` (no summaries), `2` (no develop server), Enter (apply). There is no auto-memory
question: opencode has no memory feature of its own. The summaries step names opencode's own
headless command with a free model, so its estimate is $0.00.

```text
3
2
2
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
Choose [1]: 
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
Choose 1-2: 
Step 4 of 5 · The develop server
  A second MCP server with tools to inspect and reshape the memory's structure
  (schema, saved queries, indexes). Most people do not need it; its tool
  descriptions are paid on every agent turn.

  1) Yes  register graphersal-memory-develop
  2) No   not now (`graphersal-memory install --enable-develop` later)
Choose 1-2: 
Step 5 of 5 · Summary
  This is what install will do:

  1. Agents
     - opencode (one store and one daemon serve every agent chosen)
  2. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  3. Embedding model
     - the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, not meaning)
  4. Register the MCP servers and the rules file in opencode's /tmp/gmo/h/.config/opencode/opencode.json
     - graphersal-memory (use profile): add (enabled)
     - graphersal-memory-develop (develop profile): add (disabled)
     - instructions: add /tmp/gmo/h/.config/opencode/graphersal-memory.md (loaded into every session)
  5. Write the managed rule block into /tmp/gmo/h/.config/opencode/graphersal-memory.md
     - create it with the block
  6. Add the session plugin /tmp/gmo/h/.config/opencode/plugins/graphersal-memory.js
     - create it (session start: indexing + one toast line; session end: indexing)
  7. Index past sessions (raw turns, locally, in the background)
     - opencode: 0 sessions in /tmp/gmo/h/.local/share/opencode/opencode.db: 0 to index, 0 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  8. Settings file /tmp/gmo/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

Apply these changes? [Enter = apply, n = cancel]: 

Report:
  created /tmp/gmo/h/.config/opencode/opencode.json
  created /tmp/gmo/h/.config/opencode/graphersal-memory.md
  created /tmp/gmo/h/.config/opencode/plugins/graphersal-memory.js
  created /tmp/gmo/d/config.json
  summaries run through `opencode run --model opencode/big-pickle` when you switch them on (opencode only; a free model of opencode Zen; summaries.harness_command in config.json)
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

Restart running opencode sessions to load the MCP server and the plugin. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

#### `graphersal-memory status` afterwards

```text
daemon: not running (run directory /tmp/gm-wiz1; the next agent session starts it)
data directory: /tmp/gmo/d
settings: /tmp/gmo/d/config.json (daemon idle time 7200s)
sessions: 0 indexed, 0 summarized; summaries off
Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)
opencode (configuration /tmp/gmo/h/.config/opencode/opencode.json):
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: registered, disabled (`graphersal-memory install --harness opencode --enable-develop` enables it)
  rules file in instructions: installed
  rule block (/tmp/gmo/h/.config/opencode/graphersal-memory.md): installed
  session plugin (/tmp/gmo/h/.config/opencode/plugins/graphersal-memory.js): installed
  auto-memory: opencode has none (nothing to switch off)
  headless command (summaries, bench): `opencode run --model opencode/big-pickle` by default
  sessions: read from /tmp/gmo/h/.local/share/opencode/opencode.db (not there yet)
```

#### `graphersal-memory uninstall`

The store held no memories, so the export wrote nothing; with memories it writes readable
Markdown under `~/graphersal-memory-export/opencode/` (per project `MEMORY.md` and one file per
memory, `global.md` for the global ones) and says how to give opencode a file of it
(`instructions`). Every file install created is deleted, and the `plugins/` directory it made with
it.

```text
graphersal-memory uninstall (opencode)

  - export 0 memories of 0 projects (0 global) into readable Markdown files under /tmp/gmo/h/graphersal-memory-export/opencode (opencode has no file memory; nothing is deleted)
  - /tmp/gmo/h/.config/opencode/opencode.json: delete the file (install created it)
  - /tmp/gmo/h/.config/opencode/graphersal-memory.md: delete the file (install created it)
  - /tmp/gmo/h/.config/opencode/plugins/graphersal-memory.js: delete the file (install created it)

Proceed? [y/N] y

Report:
  exported nothing: the graph holds no memories
  removed /tmp/gmo/h/.config/opencode/opencode.json
  removed /tmp/gmo/h/.config/opencode/graphersal-memory.md
  removed /tmp/gmo/h/.config/opencode/plugins/graphersal-memory.js
  removed the directory /tmp/gmo/h/.config/opencode/plugins
  Your memories stay in /tmp/gmo/d (`uninstall --purge` deletes them).
```

### opencode 2.x

Captured on 2026-11-10 with opencode 2.2.4 (`@opencode/cli-darwin-arm64@2.2.4`, unpacked into a
temporary directory, shown here as `/tmp/oc2/bin/opencode`, first on `PATH`; removed afterwards)
on a throwaway home (`/tmp/gmo2/h`, shown with that path) holding only an empty
`~/.config/opencode/` directory, with no `claude` on `PATH`. Install asked `opencode --version`
and found 2.x, so step 4 of the summary writes 2.x's shape (`mcp.servers`, see below) and the
plugin step mentions no toast. Answers as for 1.x: Enter (English), `3` (built-in model), `2` (no
summaries), `2` (no develop server), Enter (apply).

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

Step 5 of 5 · Summary
  This is what install will do:

  1. Agents
     - opencode (one store and one daemon serve every agent chosen)
  2. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  3. Embedding model
     - the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, not meaning)
  4. Register the MCP servers and the rules file in opencode's /tmp/gmo2/h/.config/opencode/opencode.json (opencode 2.x)
     - graphersal-memory (use profile): add (enabled)
     - graphersal-memory-develop (develop profile): add (disabled)
     - instructions: add /tmp/gmo2/h/.config/opencode/graphersal-memory.md (loaded into every session)
  5. Write the managed rule block into /tmp/gmo2/h/.config/opencode/graphersal-memory.md
     - create it with the block
  6. Add the session plugin /tmp/gmo2/h/.config/opencode/plugins/graphersal-memory.js
     - create it (session start and end: indexing)
  7. Index past sessions (raw turns, locally, in the background)
     - opencode: 0 sessions in /tmp/gmo2/h/.local/share/opencode/opencode.db: 0 to index, 0 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  8. Settings file /tmp/gmo2/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

Apply these changes? [Enter = apply, n = cancel]: 

Report:
  created /tmp/gmo2/h/.config/opencode/opencode.json
  created /tmp/gmo2/h/.config/opencode/graphersal-memory.md
  created /tmp/gmo2/h/.config/opencode/plugins/graphersal-memory.js
  created /tmp/gmo2/d/config.json
  summaries run through `opencode run --model opencode/big-pickle` when you switch them on (opencode only; a free model of opencode Zen; summaries.harness_command in config.json)
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

Restart running opencode sessions to load the MCP server and the plugin. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

The configuration file it created:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "graphersal-memory": {
        "codemode": false,
        "command": ["/opt/gm/bin/graphersal-memory", "mcp", "--profile", "use", "--data-dir", "/tmp/gmo2/d", "--run-dir", "/tmp/gm-wiz2"],
        "disabled": false,
        "type": "local"
      },
      "graphersal-memory-develop": {
        "codemode": false,
        "command": ["/opt/gm/bin/graphersal-memory", "mcp", "--profile", "develop", "--data-dir", "/tmp/gmo2/d", "--run-dir", "/tmp/gm-wiz2"],
        "disabled": true,
        "type": "local"
      }
    }
  },
  "instructions": [
    "/tmp/gmo2/h/.config/opencode/graphersal-memory.md"
  ]
}
```

(The commands are shortened to one line here and the binary's path to `/opt/gm/bin`; the data and
run directories appear because they were chosen explicitly.)

#### `graphersal-memory status` afterwards

```text
daemon: not running (run directory /tmp/gm-wiz2; the next agent session starts it)
data directory: /tmp/gmo2/d
settings: /tmp/gmo2/d/config.json (daemon idle time 7200s)
sessions: 0 indexed, 0 summarized; summaries off
Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)
opencode (configuration /tmp/gmo2/h/.config/opencode/opencode.json):
  version: opencode 2.x (2.2.4, from `/tmp/oc2/bin/opencode --version`)
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: registered, disabled (`graphersal-memory install --harness opencode --enable-develop` enables it)
  rules file in instructions: installed
  rule block (/tmp/gmo2/h/.config/opencode/graphersal-memory.md): installed
  session plugin (/tmp/gmo2/h/.config/opencode/plugins/graphersal-memory.js): installed
  auto-memory: opencode has none (nothing to switch off)
  headless command (summaries, bench): `opencode run --model opencode/big-pickle` by default
  sessions: read from /tmp/gmo2/h/.local/share/opencode/opencode.db (not there yet)
```

#### The smoke run that followed

On the same throwaway home: `opencode run --model opencode/big-pickle` was asked to save "We
release on Fridays" with the remember tool (it called `graphersal-memory_memories`, then
`graphersal-memory_remember`, as direct tools), and a second session was asked on which weekday
we release (it called `graphersal-memory_memories` and answered "Friday."). The plugin's start
hook queued both sessions as running; `opencode service stop` (2.x's background server) ran the
plugin's cleanup, whose end hook got both indexed (`2 found, 2 indexed`). `uninstall --yes`
exported the memory to `~/graphersal-memory-export/opencode/global.md` and deleted the three files
install had created. A summary through `opencode run --standalone` ran in its sandbox and came
back with opencode Zen's "Free usage limit reached" (the free model's daily limit was used up by
the smoke run), reported as a failed summary.

## Claude Code and opencode together

A home with both `~/.claude/` and `~/.config/opencode/` (`/tmp/gmb/h`; the binary's path is
shortened to `/opt/gm/graphersal-memory` here). The wizard starts with "Which agents"; Enter keeps
both. Both get the same MCP server command and data directory: one store, one daemon, the same
memory of a project from either agent. Answers: Enter (both), Enter (English), `3` (built-in),
Enter (switch Claude Code's auto-memory off), `2`, `2`, Enter.

```text
graphersal-memory install (Claude Code, opencode)
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
  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new
  ones, is written back to Claude Code's memory files and its auto-memory is switched
  back on.

  1) Yes  switch it off (autoMemoryEnabled false in /tmp/gmb/h/.claude/settings.json)  (default)
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

Step 7 of 7 · Summary
  This is what install will do:

  1. Agents
     - Claude Code and opencode (one store and one daemon serve every agent chosen)
  2. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  3. Embedding model
     - the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, not meaning)
  4. Register the MCP servers at user scope in /tmp/gmb/h/.claude.json (key mcpServers)
     - graphersal-memory (use profile): add
     - graphersal-memory-develop: not registered (off; `graphersal-memory install --enable-develop` registers it)
  5. Add the SessionStart and SessionEnd hooks in /tmp/gmb/h/.claude/settings.json
     - SessionStart: add `"/opt/gm/graphersal-memory" hook session-start --data-dir "/tmp/gmb/d" --run-dir "/tmp/gm-wiz2"`
     - SessionEnd: add `"/opt/gm/graphersal-memory" hook session-end --data-dir "/tmp/gmb/d" --run-dir "/tmp/gm-wiz2"`
     - autoMemoryEnabled: false (switch Claude Code's auto-memory off). Your rules and pinned memories still reach every session through graphersal-memory's start context: N memories, X characters (`graphersal-memory start-context` shows it). Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.
  6. Import Claude Code's existing file memory (MEMORY.md and the memory files)
     - 0 memory directories under /tmp/gmb/h/.claude/projects: 0 new and 0 changed memory files to import, 0 unchanged
     - The files are read, not changed; every memory stays in its project. CLAUDE.md rules are not imported in this version.
  7. Write the managed rule block into /tmp/gmb/h/.claude/CLAUDE.md
     - create the file with the block (your own text stays as it is)
  8. Register the MCP servers and the rules file in opencode's /tmp/gmb/h/.config/opencode/opencode.json
     - graphersal-memory (use profile): add (enabled)
     - graphersal-memory-develop (develop profile): add (disabled)
     - instructions: add /tmp/gmb/h/.config/opencode/graphersal-memory.md (loaded into every session)
  9. Write the managed rule block into /tmp/gmb/h/.config/opencode/graphersal-memory.md
     - create it with the block
  10. Add the session plugin /tmp/gmb/h/.config/opencode/plugins/graphersal-memory.js
     - create it (session start: indexing + one toast line; session end: indexing)
  11. Index past sessions (raw turns, locally, in the background)
     - Claude Code: 0 sessions in /tmp/gmb/h/.claude/projects: 0 to index, 0 still being written (indexed later)
     - opencode: 0 sessions in /tmp/gmb/h/.local/share/opencode/opencode.db: 0 to index, 0 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  12. Settings file /tmp/gmb/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.

Apply these changes? [Enter = apply, n = cancel]: 

Report:
  created /tmp/gmb/h/.claude.json
  created /tmp/gmb/h/.claude/settings.json
  created /tmp/gmb/h/.claude/CLAUDE.md
  created /tmp/gmb/h/.config/opencode/opencode.json
  created /tmp/gmb/h/.config/opencode/graphersal-memory.md
  created /tmp/gmb/h/.config/opencode/plugins/graphersal-memory.js
  Claude Code's auto-memory: off (uninstall writes every memory back to its memory files and switches it on again)
  created /tmp/gmb/d/config.json
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  imported nothing: no new or changed memory files
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

Restart running Claude Code and opencode sessions to load the MCP server and the hooks and the plugin. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

## Codex

Captured on 2026-11-10 like the outputs above, with the M7c build, on a throwaway home (shown as
`/tmp/gmc/h`; data directory `/tmp/gmc/d`, run directory `/tmp/gmc/r`, the binary's path
shortened to `/opt/gm/graphersal-memory`) holding only `~/.codex/config.toml` (`model =
"gpt-5.5"` and `[features] memories = true`) and a one-line `~/.codex/AGENTS.md`, so Codex was
the one harness found and the wizard did not ask which agents. Because Codex's own memories were
on, the wizard asked about them (with Codex's memories off, which is Codex's default, that step
is not shown). Answers: Enter (English), `3` (built-in model, no download), Enter (switch Codex's
memories off), `2` (no summaries), `2` (no develop server), `y` (apply).

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

  1) Yes  switch it off ([features] memories = false in /tmp/gmc/h/.codex/config.toml)  (default)
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

Step 6 of 6 · Summary
  This is what install will do:

  1. Agents
     - Codex (one store and one daemon serve every agent chosen)
  2. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  3. Embedding model
     - the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, not meaning)
  4. Register the MCP servers in Codex's /tmp/gmc/h/.codex/config.toml (tables mcp_servers.*)
     - graphersal-memory (use profile): add (enabled)
     - graphersal-memory-develop (develop profile): add (disabled)
     - [features] memories: false (switch Codex's memories off). Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is exported as readable Markdown files and Codex's memories are switched back on.
  5. Add the SessionStart and SessionEnd hooks in /tmp/gmc/h/.codex/hooks.json
     - SessionStart: add `"/opt/gm/graphersal-memory" hook session-start --data-dir "/tmp/gmc/d" --run-dir "/tmp/gmc/r" --harness codex`
     - SessionEnd: add `"/opt/gm/graphersal-memory" hook session-end --data-dir "/tmp/gmc/d" --run-dir "/tmp/gmc/r" --harness codex`
     - ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
  6. Write the managed rule block into /tmp/gmc/h/.codex/AGENTS.md
     - append the block (your own text stays as it is)
  7. Index past sessions (raw turns, locally, in the background)
     - Codex: 0 sessions in /tmp/gmc/h/.codex/sessions: 0 to index, 0 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  8. Settings file /tmp/gmc/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

  Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is exported as readable Markdown files and Codex's memories are switched back on.

Apply these changes? [Enter = apply, n = cancel]: y

Report:
  changed /tmp/gmc/h/.codex/config.toml (backup: /tmp/gmc/d/install/backups/2026-11-10T094254Z/config.toml)
  created /tmp/gmc/h/.codex/hooks.json
  changed /tmp/gmc/h/.codex/AGENTS.md (backup: /tmp/gmc/d/install/backups/2026-11-10T094254Z/AGENTS.md)
  Codex's memories: off (uninstall exports every memory as Markdown and switches them on again)
  created /tmp/gmc/d/config.json
  summaries run through `codex exec` when you switch them on (Codex only; the model of your Codex configuration; summaries.harness_command in config.json)
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

Restart running Codex sessions to load the MCP server and the hooks. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.

ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
```

`~/.codex/config.toml` afterwards (the block went before the first table, `[features]`; the
memories value was changed in place):

```toml
model = "gpt-5.5"

# graphersal-memory:begin fingerprint=... (managed by `graphersal-memory install`; `graphersal-memory uninstall` removes it; an edit inside is reported as a conflict)
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

(`--data-dir` and `--run-dir` appear because this run chose its directories explicitly; with the
defaults the arguments are just `["mcp", "--profile", "use"]`.)

### `graphersal-memory status` afterwards

```text
daemon: not running (run directory /tmp/gmc/r; the next agent session starts it)
data directory: /tmp/gmc/d
settings: /tmp/gmc/d/config.json (daemon idle time 7200s)
sessions: 0 indexed, 0 summarized; summaries off
Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)
Codex (configuration /tmp/gmc/h/.codex/config.toml):
  MCP server graphersal-memory (use): installed
  MCP server graphersal-memory-develop: registered, disabled (`graphersal-memory install --harness codex --enable-develop` enables it)
  hook SessionStart (/tmp/gmc/h/.codex/hooks.json): installed
  hook SessionEnd (/tmp/gmc/h/.codex/hooks.json): installed
  hook trust: NOT APPROVED YET
  ACTION NEEDED: Codex needs your approval for the two graphersal-memory hooks: open Codex, type `/hooks` and trust both graphersal-memory hooks (SessionStart and SessionEnd); without it the memory still works, but past sessions are indexed only after 30 minutes idle instead of at session end.
  rule block (/tmp/gmc/h/.codex/AGENTS.md): installed
  Codex memories: off (switched off by install; uninstall switches them on again)
  headless command (summaries, bench): `codex exec` by default
  sessions: read from /tmp/gmc/h/.codex/sessions (not there yet)
```

### `graphersal-memory uninstall`

One memory was saved first (through `mcp --no-daemon`, project `/tmp/gmc/proj`); answers `y`
(export), `y` (proceed). Afterwards `~/.codex/` was byte-identical to before the install (checked
with `diff -r`); the only new files are the export.

```text
graphersal-memory uninstall (Codex)

  - export 1 memory of 1 project (0 global) into readable Markdown files under /tmp/gmc/h/graphersal-memory-export/codex (Codex has no file memory; nothing is deleted)
  - /tmp/gmc/h/.codex/config.toml: restore the file as it was before install
  - /tmp/gmc/h/.codex/hooks.json: delete the file (install created it)
  - /tmp/gmc/h/.codex/AGENTS.md: restore the file as it was before install

Export 1 memory of 1 project into readable Markdown files under /tmp/gmc/h/graphersal-memory-export/codex? [Y/n] y

Proceed? [y/N] y

Report:
  exported 1 memory of 1 project (plus the global ones): 2 files written, 0 unchanged
    wrote /tmp/gmc/h/graphersal-memory-export/codex/-tmp-gmc-proj/tabs-not-spaces.md
    wrote /tmp/gmc/h/graphersal-memory-export/codex/-tmp-gmc-proj/MEMORY.md
  Codex has no file memory: the export is plain Markdown (per project MEMORY.md and one file per memory, the global ones in /tmp/gmc/h/graphersal-memory-export/codex/global.md). It is not loaded automatically; to give the agent a file, paste it into ~/.codex/AGENTS.md or a project's AGENTS.md. `graphersal-memory export --to-files --harness codex` writes it again.
  Codex's memories: switched on again, as before install
  changed /tmp/gmc/h/.codex/config.toml
  removed /tmp/gmc/h/.codex/hooks.json
  changed /tmp/gmc/h/.codex/AGENTS.md
  Your memories stay in /tmp/gmc/d (`uninstall --purge` deletes them).
```

## Cursor

Cursor is not run here (it needs a login); these outputs come from the M7d build on throwaway
homes. The wizard was captured from the in-process wizard test
(`the_wizard_lists_cursor_among_the_detected_agents`, paths shortened to `/tmp/gmu/h` for the
home and `/tmp/gmu/d` for the data directory): a home with `~/.claude` and `~/.cursor`, so both
harnesses were found and the wizard asked which agents. Answers: `3` (Cursor only), Enter
(English), `3` (built-in model), `2` (no summaries), `2` (no develop server), Enter (apply). The
step count drops from 7 to 6 after the choice: the Claude Code auto-memory step does not apply to
Cursor.

```text
graphersal-memory install (Claude Code, Cursor)
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
  ...
Step 3 of 6 · Embedding model
  ...
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
  ...
Choose 1-2: 2

Step 6 of 6 · Summary
  This is what install will do:

  1. Agents
     - Cursor (one store and one daemon serve every agent chosen)
  2. Memory language
     - English only (stored and searched in English) (in the MCP instructions and the rule block; config.json memory.language)
  3. Embedding model
     - the built-in embedder hash-ngram/1 (no download; finds shared words and spellings, not meaning)
  4. Register the MCP servers in Cursor's /tmp/gmu/h/.cursor/mcp.json (key mcpServers)
     - graphersal-memory (use profile): add
     - graphersal-memory-develop: not registered (off; `graphersal-memory install --harness cursor --enable-develop` registers it)
     - Cursor asks before each MCP tool call by default; to stop being asked for the memory tools, allow them in the approval prompt or in your run mode settings (your choice; install does not change it)
  5. Add the sessionStart and sessionEnd hooks in /tmp/gmu/h/.cursor/hooks.json
     - sessionStart: add `"/opt/gm/bin/graphersal-memory" hook session-start --data-dir "/tmp/gmu/d" --harness cursor`
     - sessionEnd: add `"/opt/gm/bin/graphersal-memory" hook session-end --data-dir "/tmp/gmu/d" --harness cursor`
     - user hooks need no approval; Cursor reloads the file on save (restart Cursor if they do not load)
  6. Write the rule file /tmp/gmu/h/.cursor/rules/graphersal-memory.mdc
     - create it (front matter alwaysApply: true, and the rule block)
  7. Index past sessions (raw turns, locally, in the background)
     - Cursor: 0 sessions in /tmp/gmu/d/sessions/cursor-transcripts.json: 0 to index, 0 still being written (indexed later)
     - summaries: off (raw turns only; `graphersal-memory summaries enable` later)
  8. Settings file /tmp/gmu/d/config.json
     - write the defaults (daemon idle time 2h) and the answers

Apply these changes? [Enter = apply, n = cancel]:

Report:
  created /tmp/gmu/h/.cursor/mcp.json
  created /tmp/gmu/h/.cursor/hooks.json
  created /tmp/gmu/h/.cursor/rules/graphersal-memory.mdc
  created /tmp/gmu/d/config.json
  embedding model: the built-in hash-ngram/1 (the daemon re-embeds the memory with it in the background)
  summaries: off (raw turns are indexed; `graphersal-memory summaries enable` switches them on)
  past sessions: indexed at the next session start (or now: `graphersal-memory import --sessions`)

Restart running Cursor sessions to load the MCP server and the hooks. `graphersal-memory status` shows what is installed; `graphersal-memory uninstall` removes it.
```

### `graphersal-memory status` afterwards

From the real binary (`install --harness cursor --yes --model builtin` on a home holding only an
empty `~/.cursor`):

```text
daemon: not running (run directory /tmp/gmu/r; the next agent session starts it)
data directory: /tmp/gmu/d
settings: /tmp/gmu/d/config.json (daemon idle time 7200s)
sessions: 0 indexed, 0 summarized; summaries off
Memory:
  memory language: English only (stored and searched in English)
  embedding model: hash-ngram/1 (built-in, no download; finds shared words, not meaning)
Cursor (configuration /tmp/gmu/h/.cursor):
  MCP server graphersal-memory (use, /tmp/gmu/h/.cursor/mcp.json): installed
  MCP server graphersal-memory-develop: not registered (`graphersal-memory install --harness cursor --enable-develop` registers it)
  hook sessionStart (/tmp/gmu/h/.cursor/hooks.json): installed
  hook sessionEnd (/tmp/gmu/h/.cursor/hooks.json): installed
  rule file (/tmp/gmu/h/.cursor/rules/graphersal-memory.mdc): installed
  Cursor memories: none in a file (nothing to switch off)
  headless command (summaries, bench): none (Cursor's CLI is not used; summaries run through Claude Code, opencode or Codex when one is installed)
  sessions: 0 transcript(s) reported by the hooks (0 on disk), registry /tmp/gmu/d/sessions/cursor-transcripts.json
```

### `graphersal-memory uninstall`

One memory was saved first (through `mcp --no-daemon` in the git repository `/tmp/gmu/proj`; on
macOS `/tmp` is `/private/tmp`). Afterwards `~/.cursor` held nothing of graphersal-memory (the
empty directory was there before install); the only new files are the export.

```text
graphersal-memory uninstall (Cursor)

  - export 1 memory of 1 project (0 global) into readable Markdown files under /tmp/gmu/h/graphersal-memory-export/cursor (Cursor has no file memory; nothing is deleted)
  - /tmp/gmu/h/.cursor/mcp.json: delete the file (install created it)
  - /tmp/gmu/h/.cursor/hooks.json: delete the file (install created it)
  - /tmp/gmu/h/.cursor/rules/graphersal-memory.mdc: delete the file (install created it)

Proceed? [y/N] y

Report:
  exported 1 memory of 1 project (plus the global ones): 2 files written, 0 unchanged
    wrote /tmp/gmu/h/graphersal-memory-export/cursor/-private-tmp-gmu-proj/tabs-not-spaces.md
    wrote /tmp/gmu/h/graphersal-memory-export/cursor/-private-tmp-gmu-proj/MEMORY.md
  Cursor has no file memory: the export is plain Markdown (per project MEMORY.md and one file per memory, the global ones in /tmp/gmu/h/graphersal-memory-export/cursor/global.md). It is not loaded automatically; to give the agent a file, paste it into a project's AGENTS.md or into a rule file under ~/.cursor/rules (a .mdc file with `alwaysApply: true`). `graphersal-memory export --to-files --harness cursor` writes it again.
  removed /tmp/gmu/h/.cursor/mcp.json
  removed /tmp/gmu/h/.cursor/hooks.json
  removed /tmp/gmu/h/.cursor/rules/graphersal-memory.mdc
  removed the directory /tmp/gmu/h/.cursor/rules
  Your memories stay in /tmp/gmu/d (`uninstall --purge` deletes them).
```

A live run with Cursor is still to be done: [cursor-live-check.md](cursor-live-check.md).
