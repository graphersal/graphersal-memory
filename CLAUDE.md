# CLAUDE.md

Guidance for AI agents working in this repository. When this file disagrees with the code, the code
wins: say so, so the stale section gets fixed.

**graphersal-memory** is a local graph memory for AI coding agents, a one-click product built on
the [Graphersal](https://github.com/graphersal/graphersal) engine. It is in pre-release: the user-facing
design is in `README.md` and `docs/`, and the stage plan is in `docs/roadmap.md`.

## Layout

- `install.sh`, `install.ps1`: the one-line installers attached to every release (see Releases).
- `crates/graphersal-memory/`: the single binary crate (`graphersal-memory`). There is one crate
  until a second binary or a library user appears, with modules per area:
  - `src/main.rs`: the entry point (parse, run, exit code);
  - `src/cli.rs`: the clap command definitions and help text, plus parsing tests;
  - `src/commands.rs`: running a command (`run_cli`): `mcp` runs the shim (`--no-daemon`:
    serves in this process), `daemon` runs (`--foreground`) or starts the daemon, `install`,
    `uninstall`, `status`, `doctor`, `import`, `summaries`, `bench`, `export --to-files`, `projects`, `start-context` and the
    hidden `hook` work on Claude Code, opencode, Codex and Cursor (`Setup` from the environment, with the model download
    `Source`; `bench_options` for `bench`; `DaemonSource` reads the memories for the export);
    `ui` and `export` without `--to-files` report their planned stage and exit with code 2
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
    whose markers were lost, `set_bool` in place; every edit re-parsed and checked);
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
    `<slug>.md` as the import reads them, global ones into the agent's global memory file;
    identical files skipped, differing memory files kept with `<slug>.graphersal.md` beside
    them, a differing `MEMORY.md` kept with the missing index lines appended; opencode, Codex
    and Cursor: the same files as Markdown under `~/graphersal-memory-export/<harness>/`);
  - `src/install.rs`: the wizard (`Current` = the current answers, `default_answers` for
    `--yes`/no terminal, `ask` for the interactive steps, `plan_install`, `install`), the
    auto-memory switch (`autoMemoryEnabled`, its previous value recorded in the settings file's
    state entry, restored by uninstall), the rule block per language (`RULE_BLOCK`,
    `RULE_BLOCK_ANY`, `PREVIOUS_BLOCK_FINGERPRINTS`), `uninstall_with` (export first, then the
    removal), Claude Code's paths (`ClaudePaths`: `~/.claude.json`, `~/.claude/` or
    `$CLAUDE_CONFIG_DIR`), the plan and apply of `install` (MCP members, hook groups, the managed
    rule block `RULE_BLOCK` with its fingerprinted begin line), the state and backups in
    `<data dir>/install/`, `uninstall` (restore an untouched file from its backup, else remove our
    parts structurally), `inspect`/`status_lines`/`doctor`. The develop server is off by not being
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
    fingerprint in `<data dir>/import/claude-code.json`, write through `remember`, the import lock);
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
    `start_context` control line);
  - `src/config.rs`: `<data dir>/config.json` (`daemon.idle`, `memory.language`,
    `memory.start_context_chars`,
    `embedding.model`, `summaries.enabled|harness_command|monthly_budget_usd|since`,
    `store.checkpoint_after|keep_history`;
    `with_summaries` and `with_values` edit in place); flag > env > file > default;
  - `src/transcript.rs`: the Claude Code transcript parser (turns, title, times, branches, edited
    files, compaction summaries; sidechains and generated records left out; unknown records,
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
    `sessions_status`;
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
    `report.md`/`report.json` in `bench/<timestamp>/`;
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
  - `src/fsutil.rs`: atomic writes and the FNV-1a fingerprint;
  - `src/ipc.rs`: the run directory (`--run-dir`, `GRAPHERSAL_MEMORY_RUN_DIR`, `<data dir>/run`
    for an explicit data directory, the platform's default), its files (`daemon.sock`,
    `daemon.pid`, `daemon.start.lock`, `daemon.log`), the socket types per platform (std on
    Unix, `uds_windows` on Windows), `LockFile` (`fd-lock`);
  - `src/protocol.rs`: the shim <-> daemon handshake and control lines, refusal codes, version
    order, `StartContextMode` (the handshake's optional `start_context`: `full` | `short` (Claude
    Code without our hook: within `CLAUDE_CODE_INSTRUCTIONS_CHARS`) | `none` (Claude Code with
    our SessionStart hook, `commands::start_context_mode`; the product's own connections)) (the
    protocol is internal; bump `PROTOCOL` on an incompatible change);
  - `src/shim.rs`: `connect_or_start` (start lock, stale socket only after a failed connect AND a
    free `daemon.pid`, detached spawn, timeout naming the log; on Windows the spawning process's
    standard handles are made non-inheritable first, `keep_std_handles_from_children`, one of the
    two `unsafe` blocks of the crate), the handshake with retries on
    `restarting`/`stopping`, then a byte relay (`run_with`, with optional `Events`: the session
    start's work on a thread after the handshake, the end's after the input closed; notes to
    stderr, never stdout); `control` (status/stop); `exchange` reads a one-line answer of up to
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
    per-user directory) and the default store `<data dir>/stores/default`, created on first use;
  - `src/catalog.rs` + `src/catalog/`: the product catalog built into the binary: `schema.json`,
    `queries.json` (name, description, parameters of each memory function) and one
    `<name>.rhai` body per function (`pin` sets `pinned`/`order`) (folder `memory`, `meta` `{"origin": "product", "catalog":
    <fingerprint>}`); `apply` writes it to a store whose product queries carry another
    fingerprint; `use_policy` builds the `use` access policy from the functions' inferred effects;
  - `src/mcp.rs` + `src/mcp/{use,develop}.md`: the two profiles (configuration, instructions,
    tool lists), the project of a session (`--project`, else the git root of the cwd),
    `StoreRuntime` (one open store: `graphersal-serve`'s session thread, one `Context` per
    profile over it sharing one `Mcp` endpoint, `serve_with` (the connection's memory language and
    start context, `Connection`: the instructions end with the project's start context, computed
    when the connection opens), `start_context` (the internal function, rendered),
    `switch_embedder` (import the cached model into the store, then rules off, old vectors
    dropped, rules on with the model: the engine checks old vectors against a new dimension
    before the fill), the job ticker (`JobTicker`, stopped by `close`,
    which then waits for `background_runs` to reach 0 before it closes the store), `serve` of one
    connection through `graphersal-mcp`'s `serve_lines` with the connection's project bound by
    the engine (`Profile::with_bound_params`: `project` is hidden in `tools/list`, set on every
    call of a saved query that declares it, left out of the answer's `call:` line), `close`),
    and `serve` (`--no-daemon`);
  - `src/mcp_tests.rs`: MCP round trips over in-memory pipes with temporary data directories
    (also the memory language in the instructions and project identity across moves/clones).
  - `src/model_tests.rs`: the model download from a local fake Hugging Face server (tiny
    synthetic Model2Vec model), install's model step, the store switching its vector rules to the
    model and back (`StoreRuntime::switch_embedder`).
  - `src/install_tests.rs`: install/uninstall/import/status/doctor/hooks on temporary homes with
    the store served in the test process (byte-identical round trips, idempotence, dry run,
    conflicts, structural uninstall, develop on/off, import of a fixture memory directory).
  - `tests/support/mod.rs`: what the end-to-end tests share across platforms (the socket type,
    `alive`/`kill` through `kill` or `tasklist`/`taskkill`, `canonical` without `\\?\`, the
    Windows variables a cleared environment keeps).
  - `tests/daemon_tests.rs`: the real binary as shims and daemon (every platform; the permission
    test Unix only): shared daemon,
    parallel calls, idle exit, stale socket, killed daemon, start race, handshake errors, upgrade
    and drain, `daemon`/`status`, permissions. Each test has its own temporary data/run/home
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

- Labels `memory`, `session`, `turn`, `fact`; edges `relates_to`, `supersedes`, `about`,
  `contains`, `edited` (`src/catalog/schema.json`, mode `open`; `emb` is a vector rule over
  `embed_text`; the schema file names the built-in `hash-ngram/1`, `catalog::apply` keeps the
  embedder the store has, and the daemon switches it to `embedding.model` of `config.json`).
- Project scoping: a vertex with a `project` property (the project's stable id, `crate::projects`)
  is visible only from that project; one without it is global. Memory ids are
  `memory:<slug>@<project>` (project) and `memory:<slug>` (global); `memory`, `forget` and
  `links` look in the project first, then in the global memory. `pinned` (bool) and `order`
  (int) select and order the start context (`crate::start_context`); `remember`'s `pinned`
  default `null` keeps the memory's pin; `pin` is a `use` tool.
- Sessions: `session:<transcript name>` (with `fingerprint` of its indexed lines, `summarized` =
  the fingerprint its summary came from, `harness`), `turn:<session>:<n>`, `fact:<session>:<kind>:<i>`
  (`fact_kind` decision | correction | outcome | open_thread), all with the session's `project`;
  `session -contains-> turn|fact`. They are written only by the internal catalog functions
  (`src/catalog/internal.json`: `indexed_sessions`, `index_session`, `index_turns`,
  `set_summary`; plus `merge_project`, `export_memories` and `start_context`), which the daemon runs on the
  store's session thread (`catalog::call`); they are never tools of the `use` server (its tool
  list is `FUNCTIONS`).
- **No LLM call without consent**: summaries run only when `summaries.enabled` is true, only
  through the user's harness command, never from a hook; tests use a fake harness script and
  assert zero calls when disabled.
- A change to a body, a description, a parameter or the schema changes the fingerprint, so every
  store gets the new catalog at its next start. Test a body change with the round trips in
  `src/mcp_tests.rs`; descriptions stay short (they are paid on every agent turn).
- `docs/`: the user-facing documentation (why, how it works, A/B method, privacy, harnesses,
  roadmap).
- `ai/`: local working files, git-ignored, never published, never linked from published files.

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p graphersal-memory -- --help
```

CI (`.github/workflows/ci.yml`) runs the same three checks on Linux, macOS and Windows.

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
- The crate stays `publish = false`; `cargo install --git` is the source path. While the repository
  is private, release files need a signed-in member, so the one-liners work once it is public.

## Engine dependency

- The engine crates (`graphersal`, `graphersal-mcp`, `graphersal-serve`, `graphersal-session`)
  are exact crates.io pins (`=0.6.0`) in the root `Cargo.toml`, all the same version (the MCP and
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
4. **Nothing is claimed as working before it works.** The README status table and the
   command messages say "planned" until a stage lands with tests.
5. **Numbers in docs come from a measurement**, with its source, its sample size and its caveats.
6. **English** in every file: code, comments, docs and commit messages.
7. **Small public surface.** There is no library API yet. Command-line flags and output formats
   are user-facing contracts, so add them deliberately.
8. **No telemetry.** Network access only for what the user asked for: model download, install
   or update, and summaries through the user's own harness after consent.
9. **Dependencies**: add a crate only with a reason. Declare it in `[workspace.dependencies]` and
   reference it with `workspace = true`.
10. Commits follow Conventional Commits.
