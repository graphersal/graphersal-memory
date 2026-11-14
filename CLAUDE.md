# CLAUDE.md

Guidance for AI agents working in this repository. When this file disagrees with the code, the code
wins: say so, so the stale section gets fixed.

**graphersal-memory** is a local graph memory for AI coding agents, a one-click product built on
the [Graphersal](https://github.com/graphersal/graphersal) engine. The user-facing documentation is
in `README.md` and `docs/`, and what is done and planned is in `docs/roadmap.md`.

## Layout

- `install.sh`, `install.ps1`: the one-line installers attached to every release (see Releases).
- `crates/graphersal-memory/`: the single binary crate (`graphersal-memory`). There is one crate
  until a second binary or a library user appears, with modules per area:
  - `src/main.rs`: the entry point (parse, run, exit code);
  - `src/cli.rs`: the clap command definitions and help text, plus parsing tests;
  - `src/commands.rs`: running a command (`run_cli`): `mcp` runs the shim (`--no-daemon`:
    serves in this process), `daemon` runs (`--foreground`) or starts the daemon (`daemon stop`:
    the `stop` control request, marker left; `daemon restart`: the `restart` control request, no
    marker, then a new daemon; both wait until the old process is gone, `daemon_action`), `install`,
    `uninstall`, `status`, `doctor`, `import`, `summaries`, `bench`, `export --to-files`, `projects`, `start-context` and the
    hidden `hook` work on Claude Code, opencode, Codex and Cursor (`Setup` from the environment, with the model download
    `Source`; `bench_options` for `bench`; `DaemonSource` reads the memories for the export);
    `ui` runs the browser UI (`crate::ui`); `export` without `--to-files` reports its planned stage and exits with code 2
    (`--harness codex` and `--harness cursor` work wherever `claude-code` and `opencode` do;
    a Claude Code hook whose input is Cursor's, `cursor_version`, does nothing); the
    wizard is interactive when stdin and stdout are terminals;
  - `src/agents.rs`: the harness abstraction (`Kind` = claude-code | opencode | codex | cursor; `Agent`: MCP
    configuration file, its servers key and entry shape, settings and instruction files, the
    sessions location, the export directories, the auto-memory key, the headless command;
    `is_our_entry` for both entry shapes) and `ClaudeCode`; `Setup::detected` (a harness's program
    found: `Setup::program`, `PATH` plus known install locations), `leftovers` (configuration
    without the program: reported, never offered) and `installed` (what status, doctor and
    uninstall work on);
  - `src/opencode.rs` + `src/opencode/`: opencode 1.x and 2.x, one `OpenCode` agent with the
    line as data (`Major`, `Detected`, `detect`: `opencode --version` in a scratch home, else the
    database's tables, else 2.x; the hooks never ask the binary). Paths `OpenCodePaths`: XDG
    config/data, the JSONC config file install edits (2.x: no `config.json`, which becomes a
    `stale_config_files` entry), `servers_path` (1.x `mcp`, 2.x `mcp.servers`), our rules file
    listed in `instructions` (never AGENTS.md, which would shadow 1.x's `~/.claude/CLAUDE.md`
    fallback), the plugin `plugins/graphersal-memory.js` (`plugin_text`: one default export
    with 1.x's `server` and 2.x's `setup`), the sandbox of headless runs (`build_sandbox`,
    `headless_args`: `--standalone` for 2.x, `normalize_run_output` of `opencode run --format
    json`), `free_model`; `opencode/install.rs` the plan/removal/inspect/status/doctor of its
    parts (2.x entries `"disabled"` + `"codemode": false`; after an upgrade it moves our 1.x
    entries out of `config.json` and `mcp.<name>`) and its rule block texts;
    `opencode/sessions.rs` the read-only SQLite reader of both lines' tables (`units`, `read`;
    a session in both is read from 2.x only when 2.x changed it after the copy; a unit path is
    `<db>#<session id>`) with both fixture schemas in its tests;
  - `src/codex.rs` + `src/codex/`: OpenAI Codex CLI (verified against `rust-v0.167.0`; the
    module docs cite the source files): `CodexPaths` (`$CODEX_HOME` else `~/.codex`:
    `config.toml`, `hooks.json`, the global instruction file = the first non-empty of
    `AGENTS.override.md`/`AGENTS.md`, `sessions/`, the export directory), `Codex` (the `Agent`),
    the headless run (`exec_args`: `--json --ephemeral --skip-git-repo-check --sandbox
    read-only`; `sandbox_config`/`build_sandbox`: a `CODEX_HOME` of its own with the user's model
    settings and a copy of `auth.json`, every built-in tool off; `normalize_exec_output` of the
    JSONL events: tokens, no cost, $0 only for `--oss`, `local_model`); `codex/install.rs` the
    plan/removal/inspect/status/doctor (one managed TOML block with both `[mcp_servers.*]` tables,
    develop `enabled = false`; the hooks in `hooks.json` with `--harness codex`; the rule block;
    `[features] memories` switched off on request and restored; the hook trust Codex records in
    `hooks.state`, which install never writes); `codex/sessions.rs` the rollout reader
    (`sessions/YYYY/MM/DD/rollout-*.jsonl`; `user_message`/`agent_message` events, both history
    modes; sub-agent sessions left out; a unit is the rollout file, id `session:<thread id>`);
  - `src/cursor.rs` + `src/cursor/`: Cursor (verified against its documentation of 2026-11-10;
    the module docs cite the pages; no live run yet, `docs/cursor-live-check.md` is the
    checklist): `CursorPaths` (`~/.cursor`: `mcp.json`, `hooks.json`, our user rule file
    `rules/graphersal-memory.mdc`, the export directory, and the registry of transcripts
    `<data dir>/sessions/cursor-transcripts.json`), `Cursor` (the `Agent`; no headless command:
    summaries and bench refuse Cursor's CLI, `NO_HEADLESS`); `cursor/install.rs` the
    plan/removal/inspect/status/doctor (`mcpServers` members, develop only with
    `--enable-develop`; flat `sessionStart`/`sessionEnd` entries, a new `hooks.json` with
    `"version": 1`; the rule file = front matter `alwaysApply: true` + the managed block);
    `cursor/sessions.rs` the registry the hooks write (`record`, under a lock) and the tolerant
    JSONL reader of the transcripts they name (the format is undocumented: role/type
    user|assistant, text parts, `<user_query>`, `tool_use` names; the rest skipped; sub-agent
    transcripts left out); Cursor's chat database is never read;
  - `src/tomledit.rs`: minimal-change TOML edits (parse with the `toml` crate, write as text: a
    fingerprinted managed block inserted before the first table, `remove_table` for our tables
    whose markers were lost, `set_bool` in place, `set_table_bool`/`remove_table_key` for another
    tool's `[mcp_servers.<name>] enabled`; every edit re-parsed and checked);
  - `src/prompt.rs`: the wizard's console (`Console`: "Step N of M", numbered choices, the
    default on Enter, re-ask, `confirm_install` with `d` for every change; colour only on a
    terminal without `NO_COLOR`), `paint`, `action_needed` and the `SEPARATOR` the other
    commands' output shares (one style: a headline, sections after a blank line, indented
    `key: value`, `~` for the home directory through `install::tilde`/`Setup::show`);
  - `src/language.rs`: the memory language (`english` | `any`) and its rule text, filled into
    `mcp/use.md`'s `{language_rule}` and into the rule block;
  - `src/models.rs`: the offered embedding models (sizes and licences from the Hugging Face
    metadata, approximate), the model cache `<data dir>/models/<repo>`, `describe`/`check` for
    `status`/`doctor`;
  - `src/download.rs`: the Model2Vec download, COPIED from graphersal-cli's `embed_model.rs`
    (marker at its top: change both); `GRAPHERSAL_MEMORY_HF_ENDPOINT`, `HF_TOKEN`;
  - `src/export.rs`: memories back into the harness's file memory (per project `MEMORY.md` +
    `<slug>.md` as the import reads them (pinned ones under `## Pinned`), global ones into the agent's global memory file;
    identical files (or the same meaning, `import::meaning`) skipped, a file unchanged since its
    import (the import state's fingerprint) rewritten with the graph's version keeping others'
    front-matter keys, other differing memory files kept with `<slug>.graphersal.md` beside
    them, superseded memories written with a `SUPERSEDED by` first line and left out of
    `MEMORY.md`, open conflicts with a `CONFLICT with` line, a differing `MEMORY.md` kept with the missing index lines appended; opencode, Codex
    and Cursor: the same files as Markdown under `~/graphersal-memory-export/<harness>/`; global
    memories imported from another memory tool left out, `from_other_tools`);
  - `src/install.rs`: the wizard (`Current` = the current answers, `default_answers` for
    `--yes`/no terminal, `ask` for the interactive steps, `plan_install`, `install`), the
    auto-memory switch (`autoMemoryEnabled`, its previous value recorded in the settings file's
    state entry, restored by uninstall), the rule block per language (`RULE_BLOCK`,
    `RULE_BLOCK_ANY`, `PREVIOUS_BLOCK_FINGERPRINTS`), `uninstall_with` (export first, then the
    removal), Claude Code's paths (`ClaudePaths`: `~/.claude.json`, `~/.claude/` or
    `$CLAUDE_CONFIG_DIR`), the plan and apply of `install` (MCP members, hook groups, the managed
    rule block `RULE_BLOCK` with its fingerprinted begin line), the state and backups in
    `<data dir>/install/`, `uninstall` (restore an untouched file from its backup, else remove our
    parts structurally; a backup that holds our parts is never put back, `backup_holds_ours`),
    `inspect`/`status_lines`/`doctor`; `InstallLock` (`<data dir>/install/install.lock`: one
    install or uninstall at a time); `Ours` (`Setup::ours`: our MCP entries and hooks by the
    program's name, this binary or one a state file recorded; `split_program` reads a command's
    program word); `apply_with_retries` (plan again right before writing, refuse read-only files,
    plan again when a file changed meanwhile, roll back everything on another failure);
    `plan_hook_event`/`remove_hook_groups` (our hook also inside a group shared with the user's);
    other memory (M15): `other_defaults` (switched off only by `--disable-other` or a yes in the
    wizard, `ask_other_memory`), `plan_other_memory` (the switch-off folded into the harness's
    change of that file, so backup, re-plan, read-only refusal and rollback cover it),
    `record_other` (the record with the file's write), `restore_others` (uninstall), the rule
    block naming the read-only tools (`find_block_with`, `block_text_with`); `commands` adds
    `import --ignore/--unignore` (`ignore_import`) and the "Other memory" section of `status` and
    `doctor` (the current directory's project, `current_project`);
    `way_out`/`in_file` (errors name the file and `--harness`); hook commands quote with single
    quotes for `sh`, double quotes on Windows; The develop server is off by not being
    registered (Claude Code has no global disabled state). The hook form (`HookForm` in `Setup`:
    `posix` in tests, `here` from the machine): on Windows Claude Code's hook groups are the exec
    form (`command` + `args`, no shell), the other harnesses' commands the `plain` words when
    there are; `is_our_hook` recognises every form (quoted, plain, bare, PowerShell, exec);
    `git_bash_findings`/`git_bash_status` for `doctor`/`status` on Windows;
  - `src/winshell.rs`: Windows shells: `all_shell_words` (the bare name when `PATH` resolves
    it to this binary, else the forward-slash path, else the directory's 8.3 short path, for each
    argument too; lookups injected), `short_path` (`GetShortPathNameW`, the crate's second
    `unsafe`), `find_git_bash` (as Claude Code finds it: `CLAUDE_CODE_GIT_BASH_PATH`, the default
    Git locations, the `git` on `PATH`; never WSL's bash), `needs_git_bash`;
  - `src/session_sync.rs`: what the shim does at a session's start and end so that hooks are
    optional (`plan`: the installed harnesses, else the one from the environment; `start`: the
    Claude Code memory-file import of the session's project through `hook::import_project`, then
    one indexing request per harness; `end`: the requests again, only to a running daemon);
  - `src/jsonedit.rs`: minimal-change JSON text edits (insert/remove one member or element, keep
    every other byte), used for the harness's files; `claude mcp add` is not used because it
    rewrites the whole file;
  - `src/import.rs`: the Claude Code file-memory import (parse `MEMORY.md` + memory files, resolve
    the project from transcripts or by decoding the directory name, plan new/changed by
    fingerprint in `<data dir>/import/claude-code.json`, write through `remember`, the import lock;
    `<slug>.graphersal.md` files skipped, the export's first-line notes taken off, folded YAML
    descriptions read, `from_file.dir` sent so memories of one directory are not `unchecked`
    against each other, `migrate_sources` once at the daemon's start for memories imported before
    M12; `## Pinned` lines become `pinned`, the index position the memory's `order`, both sent only
    when the file's line changed since this import set them; files and directories of the state's
    `ignored` list (`import --ignore`) skipped by `plan`; a changed file that brings back a memory
    forgotten in the graph or changed there by an agent is still written and gets `returned` in
    its record, `returning_line` in the report);
  - `src/instructions.rs`: memories against the instruction files the harnesses load (the
    conflict check, `docs/how-it-works.md#memories-against-instruction-files`): `files` (the
    installed harnesses' user-level and project files from `coexist::instruction_files`),
    `rules_in`/`rules_in_file` (list items, paragraphs, quotes that are directives,
    `is_directive`: an English directive word, a rules heading, or a rules file; headings,
    tables, code, comments, front matter, link lists, our managed block and short units left
    out), `rescan_once` (the one-time drop of waiting candidates per store, marker
    `<data dir>/import/instructions-rescanned-<store>`), `rule_id` (project + relative file + text),
    `read_rules` (the daemon's side of the control request `sync_instructions`), `sync` (skips
    files unchanged since the last finished sync, `<data dir>/import/instructions.json`); called
    by the shim's session start, the Claude Code / Cursor start-context fetch and `import`;
  - `src/coexist.rs` + `src/coexist/other_import.rs`: other memory beside ours (M15; files only,
    no other server started): `detect` (`BuiltIn` per harness, `Server` = another memory tool's
    MCP entry by the known `TOOLS`, a name saying memory or `MEMORY_TOOL_NAMES` from the entry or
    Claude Code's `permissions` rules; `Plugin`; `Mention` = an instruction-file line about memory
    elsewhere, `mentions_in`; `Writer` = a generated header or imported files changed outside every
    transcript's span; `Returning` from the import records), `Choices`
    (`<data dir>/install/other-memory.json`: read-only and kept tools), the rule block's
    `read_only_line` (`rule_with`, `strip_read_only` so a block naming other tools stays ours),
    `disable_edit`/`restore_edit` per harness (Claude Code and Cursor: entry removed and kept in
    the record `OFF_RECORD`; opencode `enabled`/`disabled`; Codex `enabled = false`),
    `status_lines`/`doctor_findings` (notes, never problems or ACTION NEEDED);
    `other_import`: the reference memory server's JSONL and Basic Memory's notes as global
    memories `kg-*`/`basic-*` through a connection without a project, item fingerprints in
    `<data dir>/import/other-memory.json`;
  - `src/client.rs`: a small MCP client; `connect` opens a `use` connection to the daemon (the
    product's own calls go through the daemon, the single writer);
  - `src/hook.rs`: `hook session-start|session-end`: time-boxed import of the session project's
    changed memory files, then one `index_sessions` control line to the daemon (`run_with`'s
    `Enqueue`; start: its transcript is `active`, end: `ended`), at session start one status line
    plus the start context (`run_with_context`, `StartFetch` from the daemon's `start_context`
    control line after the import; `claude_start_output`: one JSON object with
    `hookSpecificOutput.additionalContext`, capped by `CLAUDE_CODE_CONTEXT_CHARS`; the plain line
    when there is none), always exit 0; `GRAPHERSAL_MEMORY_HOOK=off` makes it do nothing; `run_opencode` for
    opencode's plugin (`--harness opencode`: no import, `<db>#<id>` active/ended); `run_codex`
    for Codex's hooks (`--harness codex`: no import, the rollout `transcript_path`
    active/ended, the start line becomes model context); `run_cursor` for Cursor's hooks
    (`--harness cursor`: records `transcript_path` + `workspace_roots` in the registry, the
    transcript active/ended, the start answer `{"additional_context": ...}` with the start
    context of the workspace, `run_cursor_with_context`); `read_input` and
    `is_cursor_input` (a Claude Code hook run by Cursor's third-party hooks does nothing);
  - `src/start_context.rs`: the start context (D7): the always-loaded memories (pinned, feedback,
    user) of a project and the global ones, `sort` (pinned, feedback, user; `order`, newest),
    `render` within the budget (groups like `MEMORY.md`, "N more not shown"), `StartContext`
    (`status_line`, the control-line JSON), `request`/`request_running` (the daemon's
    `start_context` control line; `project_id` uses an id as it is, `entries` adds the
    always-loaded memories in selection order with tier, group, shown and `shortened` (the hook cut at `MAX_HOOK_CHARS`), `entries_json`);
  - `src/config.rs`: `<data dir>/config.json` (`daemon.idle`, `memory.language`,
    `memory.start_context_chars`,
    `embedding.model`, `summaries.enabled|harness_command|monthly_budget_usd|since`,
    `store.checkpoint_after|keep_history`;
    `with_summaries` and `with_values` edit in place); flag > env > file > default;
  - `src/transcript.rs`: the Claude Code transcript parser (turns, title, times, branches, edited
    files, compaction summaries, `memory_writes`: `memory_tool`/`memory_writes` recognise our
    memory tools in every harness's spelling (also bare names with a `slug`, Cursor) and are
    called by every harness's reader; sidechains and generated records left out; unknown records,
    bad lines and a partial last line skipped; fingerprint of the complete lines);
  - `src/sessions.rs`: the session indexing per harness (`Request.harness`; the daemon keeps one
    pending request per harness): `units` (Claude Code: `scan`, top-level `*.jsonl` only;
    opencode: its database; Codex: its rollouts; Cursor: the transcripts in its registry),
    finished = unchanged for `IDLE` (30 min) or `ended`, state
    `<data dir>/sessions/<harness>.json`,
    `Writer` (the daemon: the project's store; tests: `tests::Direct`), `run` (index in batches of
    40 turns, then summaries; a failing or panicking file is recorded in its state entry,
    `failed`, and skipped after `INDEX_ATTEMPTS` tries of the same version under the same binary
    version, the rest goes on), `estimate`, `plan`, the control lines `index_sessions` /
    `sessions_status`; the session -> memory links (`memory_links`, `link_session`: the
    transcript's `memory_writes` become `wrote` edges through the internal `link_memories`, only
    when there are writes; `backfill_links` once per state entry without `memory_links` =
    `LINKS_VERSION`);
  - `src/summaries.rs`: the opt-in summaries: the prompt and JSON schema (ported from the
    prototype), `harness_args` (`claude` gets `--output-format json --json-schema ... --tools ""
    --strict-mcp-config --no-session-persistence`), `run_harness_with` (prompt on stdin, timeout; an `opencode` command in its sandbox with
    `--format json`,
    `GRAPHERSAL_MEMORY_HOOK=off`), `parse_answer`, `Spend` (`<data dir>/sessions/spend.json`),
    `Summarizer` (exists only when enabled; budget check before each call), `facts`, `render`;
  - `src/harness.rs`: running the user's harness: `program_path` (Windows: a bare name such as
    `claude` resolved through `PATH`/`PATHEXT`, so npm's `claude.cmd` is found; std runs a
    `.cmd` through `cmd.exe` with safe quoting, so keep line breaks out of arguments) and `run`
    (stdin prompt, timeout, output);
  - `src/bench.rs` + `src/bench/`: the built-in A/B (`bench`, `bench questions`): `Options`, the
    `Harness` trait (`Process`: the real `claude -p` through `harness::run`; `DryRun`: canned,
    no cost), the estimate and the cost guard (refuse above the budget, `--max-budget-usd` per
    call, stop at the budget), the run (warm-up per arm, run id at the start of every prompt,
    repeats, a judge call per answer; none for a question without a reference), `Usage` and the
    verdict parsing; `edit_command` (`bench questions add|import|remove`, no model) and the
    optional interactive step `ask_own_questions`; `find_references` (`Kind::Reference`: the
    user's questions without an answer looked up in the same raw transcripts);
    `bench/questions.rs` the question set (format 2: `origin` user|generated, `reference_from`
    transcripts|user|pending|not_found; format 1 loads as all generated; generated from RAW
    transcripts with turn ids, never from our summaries; `bench/questions/<project slug>.json`,
    user-editable, confirmed by `--review`; a regeneration keeps the user's questions;
    `parse_import` of text, `Q:`/`A:` and JSON); `bench/arms.rs` the sandbox per arm (`mcp.json`, `settings.json`, `system.md`,
    `cwd/`; the graph arm on `Store::fork_dir` of the live store served by `mcp --no-daemon`);
    `bench/report.rs` the aggregation (per category, per origin, unjudged rows in no mean) and
    `report.md`/`report.json` in `bench/<timestamp>/`; `bench/rule_changes.rs` `bench
    rule-changes` (built-in rule changes, arms plain/marked/file, each a store of its own in the
    run directory, verdict by the answer's words, no judge call; Claude Code only);
  - `src/bench_tests.rs`: `bench` in-process with a scripted harness on a temporary home and data
    directory (the live files unchanged, the sandboxes never naming them, report numbers, budget
    refusal and stop, confirmation, dry run, the user's own questions);
  - `src/projects.rs`: project identity, `<data dir>/projects.json`: a stable id per project
    (its path at first sight; legacy entries keep their path), looked up at every connection and
    indexed session by git remote (normalized, origin first), then path; a root commit shared
    with an older project is only a hint (`look_alike_lines`, `projects merge` through the
    daemon's `merge_projects` control line and the internal function `merge_project`);
  - `src/maintenance.rs`: store maintenance (`Settings`: the checkpoint threshold the stores
    open with, default 16 MiB, and the history kept, default 30 days; `prune_bound` and
    `prune_old_history` through the engine's `Store::prune`, which the daemon's maintenance
    thread runs daily and after each new snapshot; the copy lock `stores/<name>.copy.lock` the
    bench's fork holds (`hold_for_copy`) and a prune takes without waiting; the state file
    `stores/<name>.maintenance.json`; the `status` lines and the `doctor` WAL finding);
  - `src/maintenance_tests.rs`: the automatic checkpoint at a small threshold, the prune with an
    injected clock, the copy lock, a bench fork that opens after a prune;
  - `src/fsutil.rs`: atomic writes (through a symbolic link to its final target, the link kept:
    `write_target`; unique temporary names; a read-only file refused: `not_writable`;
    `write_atomic_checked` stops when the file changed meanwhile, `CHANGED_MEANWHILE`) and the
    FNV-1a fingerprint;
  - `src/ipc.rs`: the run directory (`--run-dir`, `GRAPHERSAL_MEMORY_RUN_DIR`, `<data dir>/run`
    for an explicit data directory, the platform's default), its files (`daemon.sock`,
    `daemon.pid`, `daemon.start.lock`, `daemon.log`, `daemon.stopped` (`STOPPED_FILE`: written by a `stop`
    control request (never by `restart`), removed by the next daemon's start; `stopped_on_purpose`); a default whose socket path is too long
    becomes `short_run_dir`, `/tmp/graphersal-memory-<fingerprint of the data dir>`), the socket types per platform (std on
    Unix, `uds_windows` on Windows), `LockFile` (`fd-lock`);
  - `src/ui.rs`: `graphersal-memory ui` (stage UI): the loopback HTTP server (`tiny_http`, port 0 or
    `--port`), the per-run token (256 bits, `ring`, header only, constant-time compare), the
    Host/Origin check (`graphersal_serve::HostCheck`, loopback, this port), the launch file
    `ui-<pid>.html` in the run directory (0600; removed at the first heartbeat and on exit; older
    ones swept at start), the browser opened on the file's path only, the heartbeat exit
    (`HEARTBEAT_TIMEOUT` 15 min, hidden `--heartbeat-timeout` for tests), one `use` handshake
    held open so the daemon stays (reconnects, starting a daemon unless `shim::may_start` says no), `/api/ui/*` answered itself (`status` with the
    `status`/`doctor` lines from `commands::ui_report`, `projects`, `start-context` (the control
    line with `project_id` + `entries`), `settings` GET/POST of `memory.start_context_chars`
    only, `heartbeat`), every other `/api/` relayed (`crate::relay`), static files: the
    profile's (`crate::ui_assets`) first, then the embedded playground (`Ui::embedded`, its
    `index.html` with `/memory-ui/boot.js` added);
  - `src/relay.rs`: the relay frame (opening `{"protocol", "relay": "http", "port", "method",
    "target", "headers", "body_length"}` + body; answer line + body; only `FORWARDED_HEADERS`,
    never the token; bodies up to serve's `MAX_BODY_BYTES`), `send` (the `ui` side, through
    `shim::connect_again`: a daemon that went away is started again) and
    `selection` (the memory functions + `catalog::UI_FUNCTIONS`, never an internal one);
  - `src/ui_assets.rs` + `ui/`: the `memory` profile built into the binary (`profile.json`: tabs
    Overview, Memory, Graph, Always loaded, Conflicts, Sessions as modules, cards `memory`, `session`,
    `fact` and the Graph tab's edge cards, the `graph` key (styles per label, classes per kind /
    always loaded / imported / superseded, legend: the engine's `api.graph`, EXPERIMENTAL);
    `graph-model.js` the Graph tab's pure logic (graph data of `memory_graph` /
    `memory_neighbours`, classes, card buttons and their writes; `ui/tests/graph-model.test.mjs`);
    `boot.js` takes the token from `#token=` into sessionStorage, hands it to the engine as
    `window.GRAPHERSAL_ENGINE = {headers: {Authorization}}` (the playground's host hook; never a
    `window.fetch` wrapper; `api-ui.js` sends the same headers to `/api/ui/*`) and sends the
    heartbeat (`ui/tests/boot.test.mjs`); `ui_assets::profile` adds the binary's version as the
    profile's `version` (the header shows it); `model.js` the pure logic, tested by
    `node --test crates/graphersal-memory/ui/tests/*.test.mjs`; `topbar.js` the profile's
    `header` module (the engine's top-bar slot: the Project select, daemon state, setup-problems
    warning; `ui/tests/topbar.test.mjs`); `shared.js` the shared selection, a tab's note header,
    calls per selection ("All projects" = once per project + global once), the memory form with
    the "changed meanwhile" check and the nearest check; one module per tab);
  - `src/protocol.rs`: the shim <-> daemon handshake and control lines, refusal codes, version
    order, `StartContextMode` (the handshake's optional `start_context`: `full` | `short` (Claude
    Code without our hook: within `CLAUDE_CODE_INSTRUCTIONS_CHARS`) | `none` (Claude Code with
    our SessionStart hook, `commands::start_context_mode`; the product's own connections)); an
    empty `project.root` is no project (global memories only; the import from another memory
    tool) (the protocol is internal; bump `PROTOCOL` on an incompatible change);
  - `src/shim.rs`: `connect_or_start` (start lock, stale socket only after a failed connect AND a
    free `daemon.pid`, detached spawn, timeout naming the log; on Windows the spawning process's
    standard handles are made non-inheritable first, `keep_std_handles_from_children`, one of the
    two `unsafe` blocks of the crate), the handshake with retries on
    `restarting`/`stopping`, then a line relay (`run_with`, with optional `Events`: the session
    start's work on a thread after the handshake, the end's after the input closed; notes to
    stderr, never stdout) that survives a daemon restart: `Book` (the harness's `initialize`, its
    `notifications/initialized`, the ids in flight), on the connection's end the in-flight
    requests answered `RESTARTED_CODE`, `open_with` again until the start timeout, the opening
    replayed under a shim-internal id, `notifications/tools/list_changed` when the daemon's
    version changed; `may_start`/`connect_again` (no new daemon after one was stopped on purpose,
    `ipc::STOPPED_FILE`); `control` (status/stop); `exchange` reads a one-line answer of up to
    `MAX_ANSWER_BYTES` (an export carries every memory) and reports a missing, cut or non-JSON
    answer as "the daemon failed ...; see <daemon.log>";
  - `src/daemon.rs`: the daemon: pid lock, socket, one thread per connection, routing by the
    handshake to a `StoreRuntime` (the default store opened at the start, beside the first
    handshake; others on first use) and its profile, the maintenance thread, idle exit (no exit while
    the indexer or a store's scheduled-job runs work), drain/restart for a newer shim, clean
    shutdown (socket first, then the stores; the process exit afterwards only ends the blocked
    accept loop); panics: a hook logs each one with its thread and what it was doing (`doing`:
    the control request, catalog function or session file, never memory contents; the message
    cut at its first quote, `panic_text`), connection and control handling and the indexer run
    under `catch_unwind` (a control request gets an `internal` refusal naming the log), a job
    panic is caught by the engine, and a store whose session thread ended makes the daemon exit
    cleanly (`check_alive`); debug builds only: `GRAPHERSAL_MEMORY_TEST_PANIC=<site>:<name>`
    (`injected_panic`, sites `control`, `job`, `index`) for the tests;
  - `src/registry.rs`: `registry.json` (project -> store; everything goes to `default` until
    isolation, M7);
  - `src/engine.rs`: the pinned engine version (`engine_version!`, shown by `--version`), and
    tests that exercise the engine crates and check the pin;
  - `src/store.rs`: the data directory (`--data-dir`, `GRAPHERSAL_MEMORY_DIR`, the platform's
    per-user directory; `OUR_ENTRIES`, the only entries purge deletes; `foreign_data_dir`, which
    install refuses) and the default store `<data dir>/stores/default`, created on first use;
  - `src/catalog.rs` + `src/catalog/`: the product catalog built into the binary: `schema.json`,
    `queries.json` (name, description, parameters of each memory function) and one
    `<name>.rhai` body per function (`pin` sets `pinned`/`order`), `ui.json` the browser UI's
    functions (`UI_FUNCTIONS`: `overview`, `memory_list`, `set_order`, `session_list`,
    `memory_graph`, `memory_neighbours` (the Graph tab); no tools of
    any MCP server, `ui_policy` adds their effects) (folder `memory`, `meta` `{"origin": "product", "catalog":
    <fingerprint>}`); `apply` writes it to a store whose product queries carry another
    fingerprint; `use_policy` builds the `use` access policy from the functions' inferred effects;
  - `src/mcp.rs` + `src/mcp/{use,develop}.md`: the two profiles (configuration, instructions,
    tool lists), the project of a session (`--project`, else the git root of the cwd),
    `StoreRuntime` (one open store; `relay`: the UI's relay context per `ui` port, allow-list
    mode with `relay::selection` and `ui_policy`, the profile from `ui_assets`, at most
    `MAX_RELAYS`; `graphersal-serve`'s session thread, one `Context` per
    profile over it sharing one `Mcp` endpoint, `serve_with` (the connection's memory language and
    start context, `Connection`: the instructions end with the project's start context, computed
    when the connection opens), `start_context` (the internal function, rendered),
    `switch_embedder` (import the cached model into the store, then rules off, old vectors
    dropped, rules on with the model: the engine checks old vectors against a new dimension
    before the fill), the job ticker (`JobTicker`, stopped by `close`,
    which then waits for `background_runs` to reach 0 before it closes the store), `serve` of one
    connection through `graphersal-mcp`'s `serve_lines` with the connection's project bound by
    the engine (`Profile::with_bound_params`: `project` and `source` (= `AGENT_SOURCE`, so an agent
    never writes as the UI) are hidden in `tools/list`, set on every
    call of a saved query that declares it, left out of the answer's `call:` line), `close`),
    and `serve` (`--no-daemon`);
  - `src/mcp_tests.rs`: MCP round trips over in-memory pipes with temporary data directories
    (also the memory language in the instructions and project identity across moves/clones).
  - `src/model_tests.rs`: the model download from a local fake Hugging Face server (tiny
    synthetic Model2Vec model), install's model step, the store switching its vector rules to the
    model and back (`StoreRuntime::switch_embedder`).
  - `src/coexist_tests.rs`: other memory on temporary homes with realistic configurations of
    other tools (detection per harness, defaults, switch-off and the byte-identical give-back,
    symlinked/read-only/rollback, the wizard step, imports, instruction lines, writers, returning
    facts and `import --ignore`, the rule block with a read-only line, the export leaving them out);
  - `src/install_tests.rs`: install/uninstall/import/status/doctor/hooks on temporary homes with
    the store served in the test process (byte-identical round trips, idempotence, dry run,
    conflicts, structural uninstall, develop on/off, import of a fixture memory directory).
  - `tests/support/mod.rs`: what the end-to-end tests share across platforms (the socket type,
    `alive`/`kill` through `kill` or `tasklist`/`taskkill`, `canonical` without `\\?\`, the
    Windows variables a cleared environment keeps).
  - `tests/daemon_tests.rs`: the real binary as shims and daemon (every platform; the permission
    test Unix only): shared daemon,
    parallel calls, idle exit, stale socket, killed daemon, start race, handshake errors, upgrade
    and drain, `daemon`/`status`, permissions, a daemon restart under an open session (the next
    call through a new daemon, an in-flight request answered with an error (Unix: SIGSTOP),
    the session start not repeated, a version change announced, a `stop` not undone). Each test has its own temporary data/run/home
    directories (`GRAPHERSAL_MEMORY_DIR`, `GRAPHERSAL_MEMORY_RUN_DIR`, short
    `GRAPHERSAL_MEMORY_IDLE`) and stops (or kills) every daemon it started.
  - `tests/install_tests.rs`: the real binary's install/status/doctor/hook/uninstall/purge on a
    temporary home (environment cleared; every platform), the settings file's idle time, and the
    installed `claude` binary reading the registration (`claude mcp get`, its HOME pointed at
    the temporary home; skipped when `claude` is missing).
  - `tests/sessions_tests.rs`: the real binary's hooks, `import --sessions` and `summaries` on a
    temporary home with fixture transcripts (their time set a day back) and a fake `claude`
    shell script that records its calls; asserts zero calls without consent (Unix only: the
    fake harnesses are shell scripts, as in the summaries tests of `src/sessions.rs`).
  - `tests/ui_tests.rs`: the real binary's `ui --no-browser --port 0` and its daemon on temporary
    directories over plain HTTP (std `TcpStream`): token 401, Host/Origin 403, no CORS headers,
    GETs leave the commit position unchanged, the relay refuses internal functions, a round trip
    create -> list -> edit -> pin -> set_order -> preview -> budget -> settle -> forget, imported
    memories refused, the launch file (0600, gone at the first heartbeat), the heartbeat exit with
    the daemon kept alive meanwhile, `/api/...` answered after a daemon restart.
  - `tests/bench_tests.rs`: the real binary's `bench` with a fake `claude` on `PATH` that records
    its calls (dry run: zero calls; a run through the harness: the flags of every call, the live
    store and the sources unchanged, the spend in the report; Unix only, shell-script fakes).
- Windows: paths a harness recorded are text of maybe another platform
  (`transcript::is_windows_path`, `join_text`, `relative_file`: either separator, no case); the
  home directory is `store::home_dir` (`USERPROFILE` first on Windows); a Unix-style fixture
  path in a unit test goes through `mcp_tests::abs` (a leading `/` is not absolute on Windows);
  build compared paths with `Path::join`, never `join("a/b")`.
- `install.rs` holds Claude Code and the shared wizard; each other harness has its own
  `<harness>/install.rs`.

## Memory model

- Labels `memory`, `session`, `turn`, `fact`, `instruction` (a line of an instruction file, only
  for the conflict check: never found by `recall`/`memories`); edges `relates_to`, `supersedes`,
  `conflicts_with`, `narrows`, `about`, `contains`, `edited`, `wrote`, `compared_with`
  (instruction -> memory, `decision` unchecked|compatible|conflict; written by the internal
  `sync_instructions`/`scan_instructions`, by `remember` for a memory whose text changed, and by
  `settle(slug, compatible|conflict, rule: <id>)`) (`src/catalog/schema.json`, mode `open`; `emb` is a vector rule over
  `embed_text`; the schema file names the built-in `hash-ngram/1`, `catalog::apply` keeps the
  embedder the store has, and the daemon switches it to `embedding.model` of `config.json`).
- Project scoping: a vertex with a `project` property (the project's stable id, `crate::projects`)
  is visible only from that project; one without it is global. Memory ids are
  `memory:<slug>@<project>` (project) and `memory:<slug>` (global); `memory`, `forget` and
  `links` look in the project first, then in the global memory. `pinned` (bool) and `order`
  (int) select and order the start context (`crate::start_context`); `remember`'s `pinned`
  default `null` keeps the memory's pin; `pin` is a `use` tool. `remember`/`forget` take `source`
  (`remember` | `ui`; bound to `remember` on MCP connections): the UI's memories are stored with
  `source` "ui" and reported as "ui" (never as a file); `source` "ui" refuses to edit or forget an
  imported memory.
- Conflicts (M12, `docs/how-it-works.md#conflicts-between-memories`): `remember` answers
  `nearest` (the `recall` scoring on its slug + summary, k=5, without itself and superseded ones)
  and a `check` text only when something is near; `remember(supersedes: [slug])` and
  `settle(slug, compatible|narrows|supersedes|conflict, other)` write the edges (each with
  `at`); `conflicts()` lists open conflicts, `unchecked` imported memories and stale files. A
  superseded memory is marked "SUPERSEDED by <slug> on <date>:" by `memories`, `recall` and
  `memory` (with its `chain`), never hidden, and left out of `start_context`; the internal
  `open_conflicts` feeds the start context and `status`/`doctor`. Imports pass `from_file`
  (`source` = the file, `imported_at`, `updated_at` = the file's mtime). Storage holds edges and
  properties only, never text markers (the UI reads them).
- Sessions: `session:<transcript name>` (with `fingerprint` of its indexed lines, `summarized` =
  the fingerprint its summary came from, `harness`), `turn:<session>:<n>`, `fact:<session>:<kind>:<i>`
  (`fact_kind` decision | correction | outcome | open_thread), all with the session's `project`;
  `session -contains-> turn|fact`. They are written only by the internal catalog functions
  (`src/catalog/internal.json`: `indexed_sessions`, `index_session`, `index_turns`,
  `set_summary`; plus `merge_project`, `export_memories`, `start_context`, `open_conflicts`,
  `migrate_import_sources`, `link_memories`, `sync_instructions` and `scan_instructions`), which the daemon runs on the
  store's session thread (`catalog::call`); they are never tools of the `use` server (its tool
  list is `FUNCTIONS`).
- **No LLM call without consent**: summaries run only when `summaries.enabled` is true, only
  through the user's harness command, never from a hook; tests use a fake harness script and
  assert zero calls when disabled.
- A change to a body, a description, a parameter or the schema changes the fingerprint, so every
  store gets the new catalog at its next start. Test a body change with the round trips in
  `src/mcp_tests.rs`; descriptions stay short (they are paid on every agent turn).
- `docs/`: the user-facing documentation (why, how it works, A/B method, privacy, harnesses,
  install, roadmap, the Cursor live check); `docs/images/`: the README's screenshots (a demo
  project with invented memories, light and dark).

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p graphersal-memory -- --help
```

CI (`.github/workflows/ci.yml`) runs the same three checks on Linux, macOS and Windows; it is
started by hand (`workflow_dispatch`).

## Releases

- `.github/workflows/release.yml`: on a pushed tag `v<version>` (it must equal the workspace
  version), or `workflow_dispatch` (input `dry_run`, default true: build, smoke-test, checksum,
  installer tests, nothing published). It builds `graphersal-memory` with the `dist` profile for
  linux-x86_64 / linux-aarch64 (static musl), macos-arm64, macos-x86_64 (cross-compiled on the
  arm64 runner, smoke-tested through Rosetta 2) and windows-x86_64; smoke-tests each (`--version`,
  `--help`, an MCP session of `mcp --no-daemon` over stdin on a temporary data directory); packs
  `graphersal-memory-v<version>-<platform>.tar.gz` (Windows `.zip`) with LICENSE, NOTICE,
  README.md; writes `SHA256SUMS` (archives + installers); runs both installers against the files
  served like a release by a local HTTP server; and for a real release adds build provenance
  attestations (public repository only) and creates a DRAFT GitHub Release. Never auto-publish.
- `install.sh` (POSIX sh, shellcheck-clean; macOS/Linux) and `install.ps1` (Windows PowerShell
  5.1 and 7): find the archive and its checksum in the release's `SHA256SUMS`, verify, install into
  `~/.local/bin` / `%LOCALAPPDATA%\Programs\graphersal-memory` (`--dir`/`-Dir`), print the next
  step. `--version`/`-Version`, `--base-url`/`-BaseUrl` (tests, mirrors) and the matching
  `GRAPHERSAL_MEMORY_*` environment variables. They change nothing but the binary.
- The crate is published to crates.io (`cargo install --locked graphersal-memory`); `cargo
  install --git` works too. The one-liners download from the latest published (non-draft)
  GitHub Release.

## Engine dependency

- The engine crates (`graphersal`, `graphersal-mcp`, `graphersal-serve`, `graphersal-session`)
  are exact crates.io pins (`=<version>`) in the root `Cargo.toml`, all the same version (the MCP and
  serving crates make no stability promise during 0.x). When you move the pin, update
  `engine_version!` in `src/engine.rs` (a test checks it against `Cargo.toml` and `Cargo.lock`).
- **Every engine gap goes to the graphersal repository**, as an issue or a branch there. Never
  work around it by copying engine code into this repository. graphersal-memory uses only the
  engine's public API, and it is the first external judge of that API.

## Rules

1. **No panics on user paths.** No `unwrap`/`expect`/`panic!` outside tests; clippy denies them
   (`[workspace.lints.clippy]`). Every failure becomes an error with a message that says what to
   do. Exit codes: `0` ok, `1` the command failed, `2` invalid invocation or not implemented yet.
2. **Tests never touch real user data.** Every test uses temporary `HOME`, harness configuration
   and data directories; a test that starts a daemon uses its own run directory and stops the
   daemon at its end (the daemon is detached: no test harness kills it for you). It NEVER reads or writes the real `~/.claude`, the user's stores or
   transcripts, or a running daemon or MCP server of the user. Tests make no network calls and
   call no model; LLM-dependent paths are tested with a fake harness binary.
3. **The user's live configuration is never changed by a benchmark** (every `bench` arm is a
   sandbox in the run directory; the graph arm works on a fork of the store), and install only
   writes managed, marked parts. `uninstall` must restore the configuration byte for byte, and a test
   proves it (`install_tests.rs`, both of them). Tests never write under the real `~/.claude`:
   an in-process test builds its `Setup` from a temporary home (`install_tests::setup_in`) and
   never calls `run_cli` for install/status (that reads the real environment).
4. **Nothing is claimed as working before it works.** The README, `docs/roadmap.md` and the
   command messages say "planned" until the feature lands with tests, and the README's known
   limits name what is built but not yet run live.
5. **Numbers in docs come from a measurement**, with its source, its sample size and its caveats.
6. **English** in every file: code, comments, docs and commit messages.
7. **Small public surface.** There is no library API yet. Command-line flags and output formats
   are user-facing contracts, so add them deliberately.
8. **No telemetry.** Network access only for what the user asked for: model download, install
   or update, and summaries through the user's own harness after consent.
9. **Dependencies**: add a crate only with a reason. Declare it in `[workspace.dependencies]` and
   reference it with `workspace = true`.
10. Commits follow Conventional Commits.
