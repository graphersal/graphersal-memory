# Privacy

> Pre-release: this page states the design commitments the product is built to. See the
> [status table](../README.md#status).

Agent transcripts can contain anything: code, credentials pasted by mistake, customer names.
graphersal-memory is designed so that nothing leaves your machine unless you ask for it.

## Where your data lives

- **Your per-user data directory** (macOS `~/Library/Application Support/graphersal-memory`, Linux
  `$XDG_DATA_HOME/graphersal-memory`, Windows `%LOCALAPPDATA%\graphersal-memory`). It is not in
  your repository, so it is not committed by accident.
- **No server and no ports.** The daemon listens on a local socket that only your OS user can
  open. The optional browser UI listens on loopback, and only while you run `graphersal-memory ui`.
- **Isolated stores** (planned) keep a project's memory in a store of its own, which you can hand
  over or delete on its own.

## What leaves your machine

| What | When | Where it goes |
|---|---|---|
| Nothing | by default | - |
| Session summaries | only if you opt in, once per finished session (see below) | through **your own harness** (for example `claude -p` on your own subscription), to the model provider you already use |
| Embedding model download (planned) | only if you accept it during install | a one-time download of potion-multilingual-128M; the built-in embedder needs none |
| Binary install or update | when you run the installer | GitHub Releases or crates.io |

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

## Removing it

`graphersal-memory uninstall` removes what `install` added to your harness configuration.
`graphersal-memory uninstall --purge` also deletes the data directory, which holds your graph
memory (it asks first). `install` reads your memory files but never changes or deletes them.
