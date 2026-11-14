# CodeWhale: the live check

graphersal-memory's CodeWhale support is built from CodeWhale's source (release v0.10.2) and
tested against files in its formats. One live run on macOS (2026-11-14, CodeWhale 0.10.2, a local
`gpt-oss-20b`) passed steps 2 to 7 except the write of step 5 (see `harnesses.md`, "CodeWhale");
this checklist is the run that changes "preview" into "supported" on more machines. It takes about 20 minutes and needs the
`codewhale` binary (any install method) and a model: a local one is enough (for example LM Studio
or Ollama, provider `ollama` with its base URL), so no key and no cost; load a local model with a
context of 32,768 tokens or more (8,192 is too small for CodeWhale's prompt).

To keep your real setup out of it, run everything with a throwaway home and data directory:

```sh
export HOME=$(mktemp -d) GRAPHERSAL_MEMORY_DIR=$(mktemp -d)
```

(and configure the model in that home's `~/.codewhale/config.toml`). On your own home, note first
whether `~/.codewhale/mcp.json`, `config.toml` and `AGENTS.md` exist: uninstall puts them back
exactly as they were, and step 9 checks that.

## Steps

1. **Version.** `codewhale --version`: note it (built against 0.10.2).
2. **Install.** `graphersal-memory install --harness codewhale` (defaults are fine; `--model
   builtin` avoids the download). Save the output. Check `mcp.json` (our two servers, the
   develop one `"disabled": true`), `config.toml` (`[tools] always_load` with
   `mcp_graphersal-memory_*`) and `AGENTS.md` (the marked block).
3. **MCP server.** Start `codewhale` in a git repository and run `/mcp`. Is `graphersal-memory`
   connected with its ten tools, and `graphersal-memory-develop` shown as off? If not, copy
   `/mcp validate`.
4. **Tools loaded up front.** Ask: "Which memory tools do you have?" Does the model name
   `mcp_graphersal-memory_memories` and the others without a `tool_search` first?
5. **Memory round trip.** Ask: "Remember that in this repository we use tabs, never spaces."
   Does it call `mcp_graphersal-memory_remember` (CodeWhale asks once per tool per run; choose
   "Allow for this conversation")? Then in a shell:
   `graphersal-memory start-context` lists the memory (it is a feedback memory).
6. **Rules and start context.** Quit, start `codewhale` again, ask: "How do we indent here?"
   Does it answer from the memory, without being told to search it? (The session file shows
   what arrived: `mcp_server_instructions` in `~/.codewhale/sessions/<id>.json`.)
7. **Session indexed.** Quit CodeWhale (Ctrl+C twice or `/quit`), wait a few seconds, then
   `graphersal-memory status`: does "sessions" count the CodeWhale session, and does
   `graphersal-memory status` show it under the indexed sessions? In a new session, ask the model
   to call `sessions("tabs")`.
8. **Native memory (optional).** With `[memory] enabled = true` and an entry reviewed in its
   `/memory` view, run install again: does it offer to switch the memory off, and import the
   entry as `codewhale-...`?
9. **Uninstall.** `graphersal-memory uninstall --harness codewhale`: are the three files back
   as they were (or gone, if install created them), and is the export under
   `~/graphersal-memory-export/codewhale/`?

Send the outputs, the CodeWhale version and anything that differed to a GitHub issue.
