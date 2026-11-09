# Roadmap

graphersal-memory is built in stages, strictly in order. Each stage ships with tests, and the
[status table](../README.md#status) is updated as stages land.

| Stage | What you get |
|---|---|
| **M0** (done) | This repository: what the product is, how it will work, how to measure it. The command set exists; commands of later stages report "not implemented yet". |
| **M1** (done) Memory core | A memory store with the memory tools (search, read, remember, forget, search past sessions), project and global memories, and the `use` and `develop` MCP profiles. One process serves one store over stdio, which is enough to try it by hand (`graphersal-memory mcp --no-daemon`, see [Try it](../README.md#try-it)). |
| **M2** (done) Daemon | Several agent sessions share one memory at once. A small background daemon starts on demand and stops after two idle hours, with no ports and no setup. Upgrades and crashes are handled without your help. |
| **M3** (done) One-command install (Claude Code) | `graphersal-memory install` registers the MCP server, adds the hooks, imports your existing memory files, and writes the managed rule block. `uninstall` reverts exactly that, and `status` and `doctor` tell you what is going on. Still to come: the import of `CLAUDE.md` rules, the export of rules kept in the graph, and the model download. |
| **M4** (done) Past sessions | Your finished Claude Code sessions are indexed in the background: raw turns, and opt-in summaries with decisions, corrections, outcomes and open threads, with a cost estimate and a monthly budget cap. |
| **M5** (done) Built-in A/B | `graphersal-memory bench`: compare no memory, your file memory and graphersal-memory on questions generated from your own raw sessions, sandboxed, judged, with a cost estimate, a budget and a dry run that spends nothing. Still to come: an arm with your whole current setup (another memory tool included). |
| **M6** Releases | Binaries for macOS (Apple silicon and Intel), Linux (x86_64 and aarch64) and Windows, the one-line installers and `cargo install`. |
| **M7** Later | opencode, Codex and Cursor; the browser UI for develop mode; isolated project stores; a path for people who don't use a terminal, such as a desktop app and a Claude Desktop extension. |

Ideas beyond M7: a code knowledge base as a second mode of the same tool, user-defined LLM tasks
stored in the database (for example "extract open TODOs from sessions"), and sharing a successful
memory structure with a team.
