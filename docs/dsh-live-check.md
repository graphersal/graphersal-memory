# DeepSeek Harness: the live check

graphersal-memory's DeepSeek Harness (`dsh`) support is built from dsh's source (tag
dsh-v0.2.2-rc.3, commit 38a6a5c) and tested against files in its formats on temporary homes; it
has not been run with a real dsh yet. This checklist is the run that changes "preview" into
"supported". It takes about 30 minutes and needs:

- `dsh` installed globally (`npm install -g @deepseek-ai/dsh`, Node 24.9 or newer; `npx` leaves no
  program graphersal-memory can register) or through the Desktop app;
- a DeepSeek API key (`DEEPSEEK_API_KEY`, which the `headless` profile reads at launch) or
  another provider dsh supports; a few cents of model use;
- the `graphersal-memory` binary built from this version.

**Privacy first.** dsh uploads its session log, the memory tools' results included, with
official DeepSeek API requests unless you switch it off ([privacy.md](privacy.md#deepseek-harness-dsh)).
Use made-up memories in this run, or switch the upload off first.

To keep your real setup out of it, run everything with a throwaway dsh home and data directory:

```sh
export DSH_HOME=$(mktemp -d) GRAPHERSAL_MEMORY_DIR=$(mktemp -d)
```

(Step 7 checks a moved `DSH_HOME` on purpose; this one moves it for every step.) On your own
home instead, copy `~/.dsh/cordis.patch.yml` and `~/.dsh/AGENTS.md` aside first, or note that
they do not exist: uninstall must give them back exactly, and step 10 checks that.

For each step, record the command's output (check it for paths or names you would rather not
share) and a yes/no for each question.

## Steps

1. **Version and program.** `dsh --version` and `graphersal-memory doctor`.
   Expect: doctor's "DeepSeek Harness: program ..." names your `dsh` (on Windows `dsh.cmd`).
   Record: the dsh version (built against 0.2.2-rc.3), the install method, the program line.

2. **Install.** `graphersal-memory install --harness dsh --model builtin` (defaults are fine).
   Expect: `$DSH_HOME/cordis.patch.yml` holds one marked block with one `- insert:` item and two
   rows, `graphersal-memory` and `graphersal-memory-develop` with `disabled: true`, each with
   `startupTimeoutMs: 60000` and an `env` with `GRAPHERSAL_MEMORY_HARNESS: dsh` and
   `DSH_HOME: <your temporary home>`; `$DSH_HOME/AGENTS.md` holds the marked rule block.
   Record: the install output and both files.

3. **dsh reads the rows.** `dsh --profile headless --dump-config` (the composed configuration,
   with comments naming the file each row came from).
   Expect: both rows listed, from the home layer `$DSH_HOME/cordis.patch.yml`, the develop row
   disabled; no warning about a skipped row; dsh starts without a configuration error.
   Record: the two rows' part of the dump and any warning.

4. **Tools and instructions.** In the top directory of a git repository (steps 4 to 6 run there):
   `echo "Which memory tools do you have? Answer with their exact names." | dsh --profile headless --json -`
   Expect: the answer names `mcp__graphersal-memory__memories`, `__recall`, `__remember` and the
   others; nothing in the output or in `$DSH_HOME/logs/` says the server was rejected for its
   instructions (`maxInstructionBytes`) or timed out at its start. dsh may start a short probe
   of the server before the server itself: note whether the start takes noticeably longer.
   Record: the `final` line, any `error` line, any graphersal-memory line of dsh's logs.

5. **Memory round trip.** In the same repository:
   `echo "Remember as a rule for this project: we use tabs, never spaces." | dsh --profile headless --json -`
   Expect: a `tool_call` of `mcp__graphersal-memory__remember`; then in a shell
   `graphersal-memory start-context` lists the rule (a feedback memory). Then
   `echo "How do we indent in this project?" | dsh --profile headless --json -`
   Expect: the answer says tabs, without a `tool_call` (the start context reached the model
   through the server's instructions) or after a `memories`/`recall` call (note which).
   Record: the `tool_call` lines and both `final` lines.

6. **Session indexed.** Right after step 5: `graphersal-memory status`.
   Expect: under "DeepSeek Harness (preview)", "sessions: N in .../sessions" counts the runs of
   steps 4 and 5 (each headless run is a session, `session.v4.jsonl.zstd`); a run is indexed when
   its dsh exits (the MCP connection's end), else 30 minutes after its last change. Then
   `echo "Call recall with the text tabs and the label turn, and tell me what you find." | dsh --profile headless --json -`
   Expect: a turn of step 5's session is found (`sessions` finds summaries only, which are off
   here).
   Record: the status lines, the answer, and whether `ls -R $DSH_HOME/sessions` shows any file
   besides `session.v4.jsonl.zstd` (an older generation, a `.jsonl`).

7. **A moved home.** Already covered when you set `DSH_HOME` above: in step 6 the sessions were
   those of `$DSH_HOME/sessions`, not `~/.dsh/sessions`. On your own home, repeat steps 2 and 6
   once with `DSH_HOME` set to a temporary directory.
   Record: whether status counted the moved home's sessions.

8. **Web profile with two workspaces (the known limit).** Start `dsh web` from repository A,
   open a session for repository B in the web UI and ask it to remember "B uses spaces"; then
   in a shell, in each repository: `graphersal-memory start-context`.
   Expect (the documented limit): the memory is in A's project, not B's, because one MCP server
   started in A serves both. Also note whether the web UI's MCP settings list our server once and
   without "already in use".
   Record: both start-context outputs and what the web UI showed.

9. **status and doctor.** `graphersal-memory status` and `graphersal-memory doctor`.
   Expect: every part installed, the develop server "registered, disabled", the note that dsh's
   session log upload (`session-log-deepseek`) is on, with how to switch it off naming
   `$DSH_HOME/cordis.patch.yml` (with `~` for your home). Then switch "Upload Session Log" off in
   the web UI's Settings: status and doctor should say "off in profile(s) web only; on in the
   others" (check which file dsh wrote: expected `$DSH_HOME/profiles/web/cordis.patch.yml` only).
   After adding the override to `$DSH_HOME/cordis.patch.yml` (outside our block), status says
   "off in the home layer, so in every profile" and `dsh --profile headless --dump-config` shows
   the row with `enabled: false`.
   Record: the outputs at each of the three points and the file the web toggle changed.

10. **Uninstall.** `graphersal-memory uninstall --harness dsh`.
    Expect: `cordis.patch.yml` and `AGENTS.md` byte for byte as before step 2 (`cmp` with your
    copies), or gone when install created them; when the file was dsh's empty template (a comment
    header and `[]`), it is that template again, never an empty file; `dsh --profile headless
    --dump-config` runs without our rows; the export is under `~/graphersal-memory-export/dsh/`.
    Record: the output and the `cmp` results.

11. **Windows (if you have it).** Steps 1 to 6 and 10 in PowerShell, with `$env:DSH_HOME` set to a
    temporary directory and dsh installed by npm (`dsh.cmd`) or the Desktop app.
    Expect: doctor finds `dsh.cmd`; the rows' `command` is the full path of
    `graphersal-memory.exe`; the session is indexed.
    Record: the outputs and the Windows version.

## What to report

Open an [issue](https://github.com/graphersal/graphersal-memory/issues) titled "DeepSeek Harness
live check" with the dsh version, your operating system, the records of each step, and anything
that differed from what is expected here. Never post a whole session file: its first line (the
header) with the text replaced by `...` is enough if the reader got something wrong.
