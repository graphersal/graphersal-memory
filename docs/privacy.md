# Privacy

> What graphersal-memory 0.2.1 keeps, where, and what (if anything) leaves your machine.

Agent transcripts can contain anything: code, credentials pasted by mistake, customer names.
graphersal-memory is designed so that nothing leaves your machine unless you ask for it.

## Where your data lives

- **Your per-user data directory** (macOS `~/Library/Application Support/graphersal-memory`, Linux
  `$XDG_DATA_HOME/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`). It is not in
  your repository, so it is not committed by accident. On macOS and Linux only your OS user can
  open it (mode `0700`, set when it is created and again at every daemon start), so other local
  users cannot read your memories or transcripts even when your home directory is readable.
- **No server and no ports.** The daemon listens on a local socket that only your OS user can
  open. The optional browser UI listens on loopback (`127.0.0.1`), and only while you run
  `graphersal-memory ui`; its API needs a per-run token only the opened tab has.
- **The browser UI loads libraries from public CDNs** (cdnjs.cloudflare.com and
  cdn.jsdelivr.net: Vue and the playground's other libraries, each checked against a pinned
  checksum). The CDN sees that the page was opened and from which address; no memory data, token
  or file of yours is sent there. A version without CDNs is planned.
- **Instruction files are read locally, never changed.** At a session start and on `import`,
  the daemon reads the instruction files your harness loads (`CLAUDE.md`, `AGENTS.md`, rules
  files) and keeps their lines in your store, only to compare them with your memories; nothing
  of them leaves your machine, and graphersal-memory never writes them.
- **Isolated stores** (planned) keep a project's memory in a store of its own, which you can hand
  over or delete on its own.

## What leaves your machine

| What | When | Where it goes |
|---|---|---|
| Nothing | by default | - |
| Session summaries | only if you opt in, once per finished session (see below) | through **your own harness** (for example `claude -p` on your own subscription), to the model provider you already use |
| The A/B (`bench`) | only when you run `graphersal-memory bench` and accept its estimate (see below) | through **your own harness**, like the summaries |
| Embedding model download | once, during `install`, for the model you choose (the default is a download; choose "Built-in" or `--model builtin` for none) | a download from Hugging Face (`https://huggingface.co`, or the mirror in `GRAPHERSAL_MEMORY_HF_ENDPOINT`) of the model's files, card and licence; nothing is uploaded, and `HF_TOKEN`, when set, is sent to that host only. This is the one network access of install. |
| Binary install or update | when you run the installer, `cargo install` or `graphersal-memory upgrade` | GitHub Releases or crates.io; `upgrade` asks GitHub for the latest release first, then runs the release's installer or `cargo install` (after asking you) |
| The update check | when the browser UI opens, at most once a day (the answer is kept in `<data dir>/update-check.json`); off with `"updates": {"check": false}` in `config.json` or `GRAPHERSAL_MEMORY_UPDATE_CHECK=off` | one request to `https://github.com/graphersal/graphersal-memory/releases/latest` (or `GRAPHERSAL_MEMORY_BASE_URL`), whose redirect names the latest release; the user agent `graphersal-memory/<version>` is all it says about you (GitHub sees your IP address, as with any download). This is the one network access nobody asked for each time, and it can be switched off. |
| The browser UI's libraries | each time you open `graphersal-memory ui` in the browser | the page's scripts from cdnjs.cloudflare.com and cdn.jsdelivr.net (a download; your memory stays on your machine) |

- **No telemetry**, no usage statistics and no crash reports are sent anywhere.
- **No API keys of ours, and no service of ours.** Summaries never run through an API key that
  graphersal-memory holds, and there is no graphersal-memory cloud.

## Summaries: consent, cost estimate, budget cap

Summaries make past sessions much more useful: decisions with their why, corrections, outcomes and
open threads. They also cost model usage on your account. So:

- they are **off until you agree**: `install` asks (the default answer is no, and `--yes` alone
  keeps them off; `--summaries` says yes), or `graphersal-memory summaries enable` asks later.
  The switch is `"summaries": {"enabled": true}` in `config.json` in the data directory;
  `summaries disable` turns it off again. `"since": "2026-11-01"` there limits them to newer
  sessions;
- you see a **cost estimate** before anything runs (`graphersal-memory summaries estimate`, also
  shown by `install` and `summaries enable`): the sessions without a summary and an approximate
  price at list price. It is an estimate (about four characters per token, $3 per million input
  and $15 per million output tokens, plus the harness's own prompt), not a bill;
- a **monthly budget cap** applies (`summaries.monthly_budget_usd`, $5 by default, per calendar
  month in UTC): before each call the month's spend plus the call's estimate must stay within it;
  after the call the cost your harness reports is added (`claude -p` reports it; for another
  harness the estimate is counted). Above the cap, only raw turns are indexed, which is local and
  free, and summaries catch up next month. `summaries status` shows the month's spend.

Without summaries, raw turns are still indexed locally and searchable.

### What is sent to your harness, and when

Only with summaries on, and only from the background daemon (never from a hook or an agent's
call): once per finished session (again only if the session grows later, for example when you
resume it), the daemon runs your harness command (`claude -p --model sonnet` by default,
`summaries.harness_command` in `config.json`) once and writes to its standard input:

- the instructions (summarize in English: decisions with why, corrections, outcomes, open
  threads), with the project's directory name;
- the session's title, start and end time, git branches and the paths of the files edited;
- Claude Code's own compaction summaries of the session;
- the text of every prompt and every text reply of the assistant, long ones cut in the middle.

It does not send tool calls' inputs, tool results (command output, file contents the assistant
read), the thinking blocks, or anything of sub-agents. For `claude`, the run gets no tools, no MCP
servers and no transcript of its own (`--tools "" --strict-mcp-config --no-session-persistence`),
and graphersal-memory's own hooks do nothing in it. The answer (a JSON summary) is stored in the
graph on your machine; the harness's provider handles the prompt under the terms of your own
account, as for any other session of yours.

## The A/B: what `bench` sends

`bench` runs only when you start it, shows its cost estimate first and asks (`--yes` skips the
question, `--dry-run` sends nothing at all). Through your harness command it sends:

- **to generate the questions** (once per project, or when you run `bench questions`): excerpts of
  your newest finished raw transcripts of the project, up to about 120,000 characters: the text
  of your prompts and of the assistant's text replies, each cut short, with their turn ids. Tool
  inputs, tool output and thinking are not sent. The questions, reference answers and source
  excerpts are stored in `bench/questions/` in the data directory;
- **to answer them**, per arm: for the file-memory arm your `CLAUDE.md` files (without
  graphersal-memory's marked block) and `MEMORY.md` in the prompt, with the memory files readable
  from a copy; for the graphersal-memory arm the same instruction files, and the agent reads what
  it searches for from a copy of your store; the no-memory arm gets neither;
- **to judge each answer**: the question, its reference answer and the answer.

Every call runs without your settings, hooks, plugins or MCP servers, with read-only tools, and
with no transcript of its own (`--no-session-persistence`). The reports stay in `bench/` in the
data directory. Claude Code may still update its own bookkeeping files for each run, as it does
for any `claude -p` you run; graphersal-memory changes none of your configuration.

## Removing it

`graphersal-memory uninstall` first writes every memory back into your harness's file memory
(for Claude Code: `MEMORY.md` and one file per memory in each project's memory directory, the
global ones in `~/.claude/rules/graphersal-memory-global.md`; files you changed are kept, the
graph's version goes beside them), switches the harness's own auto-memory back to how it was, and
removes what `install` added to your harness configuration (`--no-export` skips the export).
`graphersal-memory uninstall --purge` also deletes the data directory, which holds your graph
memory (it asks first). `install` reads your memory files but never changes or deletes them.
