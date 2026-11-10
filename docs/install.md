# Install and uninstall: what you see

These are real outputs, captured on 2026-11-10 by running `graphersal-memory` (a development
build) under a pseudo-terminal on a throwaway home directory (`/tmp/gmw/h`, with the data and run
directories chosen by `GRAPHERSAL_MEMORY_DIR` / `GRAPHERSAL_MEMORY_RUN_DIR`, which is why the hook
commands carry `--data-dir` and `--run-dir`). The home held one project (`/tmp/gmw/work`, a git
repository) with two Claude Code memory files and a `settings.json` of `{"theme": "dark"}`.
Colour was off (`NO_COLOR=1`); in a colour terminal the step titles are bold, the explanations
dim and the default green. An answer of Enter shows as nothing after the prompt.

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
     - autoMemoryEnabled: false (switch Claude Code's auto-memory off). Nothing is lost: if you uninstall graphersal-memory later, every memory, including new ones, is written back to Claude Code's memory files and its auto-memory is switched back on.
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
was kept and the graph's index written beside it. `settings.json` got its original bytes back
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
