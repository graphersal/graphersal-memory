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
| Session summaries | only if you opt in | through **your own harness** (for example `claude -p` on your own subscription), to the model provider you already use |
| Embedding model download | only if you accept it during install | a one-time download of potion-multilingual-128M; the built-in embedder needs none |
| Binary install or update | when you run the installer | GitHub Releases or crates.io |

- **No telemetry**, no usage statistics and no crash reports are sent anywhere.
- **No API keys of ours, and no service of ours.** Summaries never run through an API key that
  graphersal-memory holds, and there is no graphersal-memory cloud.

## Summaries: consent, cost estimate, budget cap

Summaries make past sessions much more useful: decisions with their why, corrections, outcomes and
open threads. They also cost model usage on your account. So:

- they are **off until you agree**, and you choose the scope (past sessions, new sessions only, or
  none);
- you see a **cost estimate** before anything runs;
- a **monthly budget cap** applies: above it, only raw turns are indexed, which is local and free,
  and summaries catch up when the budget allows.

Without summaries, raw turns are still indexed locally and searchable.

## Removing it

`graphersal-memory uninstall` removes what `install` added to your harness configuration. The
store is a directory in your data directory: deleting it deletes your graph memory.
