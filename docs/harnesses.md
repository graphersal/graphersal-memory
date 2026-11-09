# Supported harnesses

> Pre-release: no harness is supported yet. This page lists the targets and what each one needs.

Every harness below supports MCP, so the memory tools work the same way everywhere. What differs is
how graphersal-memory is installed into the harness, where the harness keeps its rules and
transcripts, and whether it has hooks.

| Harness | Status | MCP registration | Rules exported to | Past sessions from | Hooks |
|---|---|---|---|---|---|
| Claude Code | first target (M3, M4) | `claude mcp add`, or a careful JSON edit with a backup | a marked block in `CLAUDE.md` | local transcripts (`~/.claude/projects/`) | SessionStart / SessionEnd |
| opencode | planned (M7) | its MCP config | `AGENTS.md` (to be checked) | its local store (format to be checked) | to be checked |
| OpenAI Codex CLI | planned (M7) | its MCP config | `AGENTS.md` (to be checked) | its local store (format to be checked) | to be checked |
| Cursor | planned (M7) | its MCP config | Cursor rules (to be checked) | its local store (format to be checked) | to be checked |

Where a harness has no hooks, synchronisation runs from the MCP server itself or on demand.

## Rules for every harness

- **Only managed parts are written.** That means a marked block in the instruction file, and
  entries tagged as generated. Your own text and entries are never touched.
- If you edit inside the managed block by hand, the edit is imported back, or reported as a
  conflict. It is never silently overwritten.
- `uninstall` removes exactly the managed parts and leaves the rest as it was.

## Without a terminal (later)

Analysts, testers and managers who use desktop apps such as Claude Desktop instead of a CLI agent
are a later audience. The core is designed now so that it doesn't assume a terminal, hooks, local
transcripts or code. Rules can reach the model through the MCP server's instructions. Past
conversations can come from the services' data exports. A desktop app with an install wizard is
planned.
