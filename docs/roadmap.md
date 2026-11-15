# Roadmap

What graphersal-memory 0.2.1 does today, and what we plan next. Planned work is tracked in
[GitHub issues](https://github.com/graphersal/graphersal-memory/issues); a use case we have not
thought of is welcome there.

## In 0.2.1

| Area | What you get |
|---|---|
| Memory core | A local graph store with the memory tools over MCP (search, read, remember, forget, pin, past sessions), project and global memories, and two MCP servers: `use` (on) and `develop` (structure tools, off by default). |
| Shared daemon | Several agent sessions share one memory at once. A small background daemon starts on demand and exits after two idle hours, with no ports and no setup; upgrades and crashes are handled without your help. |
| One-command install | `install` (a short wizard) and `uninstall` for Claude Code, opencode 1.x and 2.x, OpenAI Codex CLI, Cursor and CodeWhale (new in 0.2.0, a preview), any combination on one store; `status` and `doctor`; the import of Claude Code's file memory; the export back into files on `uninstall` (`export --to-files`); a moved repository keeps its memories (`projects`). |
| Embedding model | `minishlab/potion-retrieval-32M` (English) or `minishlab/potion-multilingual-128M`, downloaded once from Hugging Face and run locally on the CPU, or a built-in embedder with no download. |
| Start context | Pinned, "how to work" and "about you" memories at the start of every session, within a character budget (8,000 by default), so a harness's own file memory can be switched off without losing your rules. |
| Conflicts | `remember` checks a new memory against its nearest neighbours; a later decision supersedes an older one and keeps its history; open conflicts and unchecked imported memories in `conflicts()`, settled with `settle`. |
| Memories against instruction files | The lines of the instruction files your harness loads (`CLAUDE.md`, `AGENTS.md`, rules files) are compared with the memories near them, never imported and never edited; confirmed contradictions show in the start context, `status`, `doctor` and the UI. |
| Past sessions | Finished sessions of every supported harness indexed in the background (raw turns), with opt-in summaries (decisions, corrections, outcomes, open threads) through your own harness, a cost estimate and a monthly budget cap. |
| Built-in A/B | `bench`: no memory, your file memory and graphersal-memory on questions generated from your own raw sessions, sandboxed and judged, with a cost estimate, a budget and a dry run; `bench rule-changes` for rules that change. |
| Browser UI | `graphersal-memory ui`: overview, memories, a graph of the memories and their decisions, the start context with its order and budget, conflicts and past sessions, on loopback with a token per run. |
| Upgrades | `graphersal-memory upgrade` through the release's installer or cargo, then `install --refresh` brings the rule block, the MCP entries and the hooks to the new version, every choice kept; "Version X available" in the browser UI (one check a day, off by a setting). |
| Store maintenance | Automatic checkpoints, history older than 30 days pruned, store sizes in `status` and `doctor`. |
| Releases | Binaries for Linux (x86_64, arm64), macOS (arm64, x86_64) and Windows (x86_64), smoke-tested on each platform, with `SHA256SUMS`, build provenance and one-line installers; `cargo install`. |

## Next

- **CodeWhale beyond the preview**: a memory written through it, Linux and Windows
  ([codewhale-live-check.md](codewhale-live-check.md)), and `bench` through `codewhale exec`.
- **Verify what is built but not yet run live**: Cursor with a live login
  ([cursor-live-check.md](cursor-live-check.md)), the harnesses on Windows, the browser UI on
  Linux and Windows, and the memory tools through Codex with a stronger local model.
- **Measure the instruction-file check** on more real instruction files, and tune its thresholds.
- **`bench` with your whole current setup**: an arm that copies your MCP servers, hooks and
  memory files (another memory tool included), so that tool is compared too, without touching its
  live data.
- **A browser UI without CDNs**, so it works offline, and more settings in it.
- **A larger measurement**: a rerun of the A/B as the memory grows, and a synthetic scale test.

## Later

- **Isolated project stores**: a project's memory in a store of its own, to hand over or delete
  on its own.
- **People who don't use a terminal**: a desktop app with an install wizard, and a path for
  desktop chat apps such as Claude Desktop.
- **Rules from the graph into instruction files**: today the always-applying memories reach the
  agent through the start context; writing them into a harness's own instruction files is
  planned (`export` without `--to-files` says so).

Ideas beyond that: a code knowledge base as a second mode of the same tool, user-defined LLM tasks
stored in the database (for example "extract open TODOs from sessions"), and sharing a successful
memory structure with a team.
