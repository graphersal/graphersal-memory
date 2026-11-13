# Roadmap

graphersal-memory is built in stages, strictly in order. Each stage ships with tests, and the
[status table](../README.md#status) is updated as stages land.

| Stage | What you get |
|---|---|
| **M0** (done) | This repository: what the product is, how it will work, how to measure it. The command set exists; commands of later stages report "not implemented yet". |
| **M1** (done) Memory core | A memory store with the memory tools (search, read, remember, forget, search past sessions), project and global memories, and the `use` and `develop` MCP profiles. One process serves one store over stdio, which is enough to try it by hand (`graphersal-memory mcp --no-daemon`, see [Try it](../README.md#try-it)). |
| **M2** (done) Daemon | Several agent sessions share one memory at once. A small background daemon starts on demand and stops after two idle hours, with no ports and no setup. Upgrades and crashes are handled without your help. |
| **M3** (done) One-command install (Claude Code) | `graphersal-memory install` registers the MCP server, adds the hooks, imports your existing memory files, and writes the managed rule block. `uninstall` reverts exactly that, and `status` and `doctor` tell you what is going on. Still to come: the import of `CLAUDE.md` rules (the model download came in M6b; the export of rules kept in the graph came in M14 as the start context: pinned, feedback and user memories at the start of every session). |
| **M4** (done) Past sessions | Your finished Claude Code sessions are indexed in the background: raw turns, and opt-in summaries with decisions, corrections, outcomes and open threads, with a cost estimate and a monthly budget cap. |
| **M5** (done) Built-in A/B | `graphersal-memory bench`: compare no memory, your file memory and graphersal-memory on questions generated from your own raw sessions, sandboxed, judged, with a cost estimate, a budget and a dry run that spends nothing. Still to come: an arm with your whole current setup (another memory tool included). |
| **M6** (done) Releases | Binaries for macOS (Apple silicon and Intel), Linux (x86_64 and aarch64, static) and Windows, built and smoke-tested on each platform by the release workflow, with `SHA256SUMS` and build provenance; the one-line installers `install.sh` and `install.ps1` that verify the checksum; `cargo install`. The binaries and installers can be downloaded once the first release (0.1.0) is published. |
| **M6b** (done) The install wizard | `install` asks a few questions in a terminal (memory language, embedding model, import, Claude Code's own auto-memory, summaries, the develop server) and shows the full plan before it changes anything; a real embedding model is downloaded and the memory re-embedded with it; `uninstall` writes every memory back into Claude Code's memory files (`export --to-files` does it alone); a moved repository keeps its memories (`projects list`, `projects merge`). |
| **M7a** Done | opencode (1.x): install/uninstall, session plugin, past sessions, summaries and bench through `opencode run`; both harnesses on one store. |
| **M7b** Done | opencode 2.x beside 1.x: the line detected with `opencode --version`, 2.x's configuration shape, one plugin file for both lines, sessions from both lines' tables, `--standalone` headless runs; `doctor` and `install` move our entries after an upgrade from 1.x. |
| **M7c** Done | OpenAI Codex CLI: MCP servers in `~/.codex/config.toml`, the rule block in `AGENTS.md`, SessionStart/SessionEnd hooks, past sessions from its rollout files, summaries through `codex exec`, bench with a local model, Codex's own memories switched off on request. |
| **M7d** Done | Cursor: the MCP server in `~/.cursor/mcp.json`, a rule file of our own in `~/.cursor/rules`, sessionStart/sessionEnd hooks, past sessions from the transcripts the hooks name; no summaries or bench through Cursor's CLI. Tested against fixtures; the live check ([cursor-live-check.md](cursor-live-check.md)) is pending. |
| **M8** Done | Store maintenance: automatic checkpoints after 16 MiB of WAL (a start replays at most about that), history older than 30 days pruned by the daemon (both in `config.json`), the default store opened at the daemon's start, store sizes and WAL in `status` and `doctor`, and the measured start time ([how it works](how-it-works.md#stores)). |
| **UI** Done | `graphersal-memory ui`: the browser UI for every user (overview, memories, a graph of the memories and their decisions, what every session starts with, conflicts, past sessions), on loopback with a per-run token, every change through the memory functions. Still to come: the libraries without CDNs, hygiene and settings beyond the start context budget. |
| **M7** Later | Isolated project stores; a path for people who don't use a terminal, such as a desktop app and a Claude Desktop extension. |

Ideas beyond M7: a code knowledge base as a second mode of the same tool, user-defined LLM tasks
stored in the database (for example "extract open TODOs from sessions"), and sharing a successful
memory structure with a team.
