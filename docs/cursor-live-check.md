# Cursor: live check

graphersal-memory's Cursor support is built from Cursor's documentation and tested against
hand-built files; it has not been run with a live Cursor yet. This checklist takes about 15
minutes. It needs Cursor (any plan, Hobby is enough) and the `graphersal-memory` binary on your
`PATH`. Nothing here sends data anywhere except Cursor's own model calls.

To keep your real setup out of it, use a test account or a machine without other Cursor
configuration. If you use your own machine, note first whether `~/.cursor/mcp.json`,
`~/.cursor/hooks.json` and `~/.cursor/rules/` exist; uninstall puts them back exactly as they
were, and step 9 checks that.

## Steps

1. **Install.** In a terminal:

   ```sh
   graphersal-memory install --harness cursor
   ```

   Answer the wizard (the defaults are fine; the built-in model avoids the download). Save the
   whole output.

2. **Restart Cursor** (quit and open it again), and open a folder that is a git repository
   (any small project).

3. **MCP server.** Open Customize (sidebar) and look at the MCP servers.
   - Is `graphersal-memory` listed, switched on, with its tools (`memories`, `recall`,
     `memory`, `remember`, `forget`, `sessions`, `session`)?
   - Is it listed once? (If graphersal-memory is also installed for Claude Code, note whether
     Cursor shows a second copy imported from Claude Code.)
   - If it shows an error, open Output (Cmd/Ctrl+Shift+U), choose "MCP Logs" and copy the lines.

4. **Rules.** In Customize, look at Rules. Is `graphersal-memory` listed (from
   `~/.cursor/rules/graphersal-memory.mdc`), always applied? (The documentation does not say how
   Customize shows user rule files; step 6 tells whether the rule reached the model either way.)

5. **Hooks.** In Customize, look at Hooks (or the Hooks output channel). Are the two
   graphersal-memory hooks (sessionStart, sessionEnd) listed, without errors?

6. **Remember.** Start a new Agent chat and type:

   > Remember for this project: we use tabs, never spaces.

   - Does the agent call `remember` (it asks for approval first; approve it)?
   - Without being told to use a tool, did it pick the memory tool by itself? (That shows the
     rule file reached the model.)
   - Note the tool name as Cursor shows it in the chat.

7. **Recall in a new chat.** Start another new chat and ask:

   > What is our indentation rule in this project?

   Does the agent search the memory (`memories` or `recall`) and answer "tabs"?

8. **Status.** In the terminal:

   ```sh
   graphersal-memory status
   graphersal-memory doctor
   graphersal-memory import --sessions
   ```

   - In `status`, under "Cursor": how many transcripts did the hooks report? (After step 7 it
     should be at least 2.)
   - `import --sessions`: how many Cursor sessions were indexed, and with how many turns?
   - If `status` reports 0 transcripts: in Cursor, check whether transcripts are on and whether
     the Hooks output channel shows our hooks running.

9. **Uninstall.**

   ```sh
   graphersal-memory uninstall --harness cursor
   ```

   Answer yes to the export. Then check that `~/.cursor/mcp.json`, `~/.cursor/hooks.json` and
   `~/.cursor/rules/` are as before step 1 (gone if they did not exist), and that the export is in
   `~/graphersal-memory-export/cursor/`. Restart Cursor: `graphersal-memory` is no longer in
   Customize.

## What to send back

- Cursor's version (Help > About, or `cursor --version`) and your operating system.
- The output of steps 1, 8 and 9.
- Yes/no for each question in steps 3 to 7, with the tool name from step 6.
- One transcript file that the hooks reported (its path is listed in
  `<data dir>/sessions/cursor-transcripts.json`; `graphersal-memory status` prints the data
  directory). Look at it first and send it only if it holds nothing private, or send just its
  first three lines with the text replaced by `...`: the reader was built without a real file,
  so the line format is what matters.
- Anything that looked wrong, with the lines from the MCP Logs or Hooks output channels.
