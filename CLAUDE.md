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
    `uninstall`, `status`, `doctor`, `import`, `summaries`, `bench` and the hidden `hook` work on
    Claude Code (`Setup` from the environment; `bench_options` for `bench`); `ui`, `export`
    report their planned stage and exit with code 2;
  - `src/install.rs`: Claude Code's paths (`ClaudePaths`: `~/.claude.json`, `~/.claude/` or
    `$CLAUDE_CONFIG_DIR`), the plan and apply of `install` (MCP members, hook groups, the managed
    rule block `RULE_BLOCK` with its fingerprinted begin line), the state and backups in
    `<data dir>/install/`, `uninstall` (restore an untouched file from its backup, else remove our
    parts structurally), `inspect`/`status_lines`/`doctor`. The develop server is off by not being
    registered (Claude Code has no global disabled state);
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
    `Enqueue`; start: its transcript is `active`, end: `ended`), one status line at session
    start, always exit 0; `GRAPHERSAL_MEMORY_HOOK=off` makes it do nothing;
  - `src/config.rs`: `<data dir>/config.json` (`daemon.idle`, `summaries.enabled|harness_command|
    monthly_budget_usd|since`, `with_summaries` edits the switch in place); flag > env > file >
    default;
  - `src/transcript.rs`: the Claude Code transcript parser (turns, title, times, branches, edited
    files, compaction summaries; sidechains and generated records left out; unknown records,
    bad lines and a partial last line skipped; fingerprint of the complete lines);
  - `src/sessions.rs`: the session indexing: `scan` (top-level `*.jsonl` only), finished =
    unchanged for `IDLE` (30 min) or `ended`, state `<data dir>/sessions/claude-code.json`,
    `Writer` (the daemon: the project's store; tests: `tests::Direct`), `run` (index in batches of
    40 turns, then summaries), `estimate`, `plan`, the control lines `index_sessions` /
    `sessions_status`;
  - `src/summaries.rs`: the opt-in summaries: the prompt and JSON schema (ported from the
    prototype), `harness_args` (`claude` gets `--output-format json --json-schema ... --tools ""
    --strict-mcp-config --no-session-persistence`), `run_harness` (prompt on stdin, timeout,
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
    repeats, a judge call per answer), `Usage` and the verdict parsing;
    `bench/questions.rs` the question set (generated from RAW transcripts with turn ids, never
    from our summaries; `bench/questions/<project slug>.json`, user-editable, confirmed by
    `--review`); `bench/arms.rs` the sandbox per arm (`mcp.json`, `settings.json`, `system.md`,
    `cwd/`; the graph arm on `Store::fork_dir` of the live store served by `mcp --no-daemon`);
    `bench/report.rs` the aggregation and `report.md`/`report.json` in `bench/<timestamp>/`;
  - `src/bench_tests.rs`: `bench` in-process with a scripted harness on a temporary home and data
    directory (the live files unchanged, the sandboxes never naming them, report numbers, budget
    refusal and stop, confirmation, dry run);
  - `src/projects.rs`: `<data dir>/projects.json`: each project's git remote (credentials removed),
    recorded by the daemon when a project first connects (for re-mapping moved repositories later);
  - `src/fsutil.rs`: atomic writes and the FNV-1a fingerprint;
  - `src/ipc.rs`: the run directory (`--run-dir`, `GRAPHERSAL_MEMORY_RUN_DIR`, `<data dir>/run`
    for an explicit data directory, the platform's default), its files (`daemon.sock`,
    `daemon.pid`, `daemon.start.lock`, `daemon.log`), the socket types per platform (std on
    Unix, `uds_windows` on Windows), `LockFile` (`fd-lock`);
  - `src/protocol.rs`: the shim <-> daemon handshake and control lines, refusal codes, version
    order (the protocol is internal; bump `PROTOCOL` on an incompatible change);
  - `src/shim.rs`: `connect_or_start` (start lock, stale socket only after a failed connect AND a
    free `daemon.pid`, detached spawn, timeout naming the log), the handshake with retries on
    `restarting`/`stopping`, then a byte relay; `control` (status/stop);
  - `src/daemon.rs`: the daemon: pid lock, socket, one thread per connection, routing by the
    handshake to a `StoreRuntime` (opened on first use) and its profile, idle exit (no exit while
    the indexer or a store's scheduled-job runs work), drain/restart for a newer shim, clean
    shutdown (socket first, then the stores; the process exit afterwards only ends the blocked
    accept loop);
  - `src/registry.rs`: `registry.json` (project -> store; everything goes to `default` until
    isolation, M7);
  - `src/engine.rs`: the pinned engine version (`engine_version!`, shown by `--version`), and
    tests that exercise the engine crates and check the pin;
  - `src/store.rs`: the data directory (`--data-dir`, `GRAPHERSAL_MEMORY_DIR`, the platform's
    per-user directory) and the default store `<data dir>/stores/default`, created on first use;
  - `src/catalog.rs` + `src/catalog/`: the product catalog built into the binary: `schema.json`,
    `queries.json` (name, description, parameters of each memory function) and one
    `<name>.rhai` body per function (folder `memory`, `meta` `{"origin": "product", "catalog":
    <fingerprint>}`); `apply` writes it to a store whose product queries carry another
    fingerprint; `use_policy` builds the `use` access policy from the functions' inferred effects;
  - `src/mcp.rs` + `src/mcp/{use,develop}.md`: the two profiles (configuration, instructions,
    tool lists), the project of a session (`--project`, else the git root of the cwd),
    `StoreRuntime` (one open store: `graphersal-serve`'s session thread, one `Context` per
    profile over it sharing one `Mcp` endpoint, the job ticker (`JobTicker`, stopped by `close`,
    which then waits for `background_runs` to reach 0 before it closes the store), `serve` of one
    connection through `graphersal-mcp`'s `serve_lines` with the connection's project bound by
    the engine (`Profile::with_bound_params`: `project` is hidden in `tools/list`, set on every
    call of a saved query that declares it, left out of the answer's `call:` line), `close`),
    and `serve` (`--no-daemon`);
  - `src/mcp_tests.rs`: MCP round trips over in-memory pipes with temporary data directories.
  - `src/install_tests.rs`: install/uninstall/import/status/doctor/hooks on temporary homes with
    the store served in the test process (byte-identical round trips, idempotence, dry run,
    conflicts, structural uninstall, develop on/off, import of a fixture memory directory).
  - `tests/daemon_tests.rs`: the real binary as shims and daemon (Unix only): shared daemon,
    parallel calls, idle exit, stale socket, killed daemon, start race, handshake errors, upgrade
    and drain, `daemon`/`status`, permissions. Each test has its own temporary data/run/home
    directories (`GRAPHERSAL_MEMORY_DIR`, `GRAPHERSAL_MEMORY_RUN_DIR`, short
    `GRAPHERSAL_MEMORY_IDLE`) and stops (or kills) every daemon it started.
  - `tests/install_tests.rs`: the real binary's install/status/doctor/hook/uninstall/purge on a
    temporary home (environment cleared, Unix only), the settings file's idle time, and the
    installed `claude` binary reading the registration (`claude mcp get`, its HOME pointed at
    the temporary home; skipped when `claude` is missing).
  - `tests/sessions_tests.rs`: the real binary's hooks, `import --sessions` and `summaries` on a
    temporary home with fixture transcripts (their time set a day back) and a fake `claude`
    shell script that records its calls; asserts zero calls without consent.
  - `tests/bench_tests.rs`: the real binary's `bench` with a fake `claude` on `PATH` that records
    its calls (dry run: zero calls; a run through the harness: the flags of every call, the live
    store and the sources unchanged, the spend in the report).
- Planned areas (one module each when they arrive): `export`, other
  harnesses (`install.rs` is Claude Code only; split it per harness when the second arrives).

## Memory model

- Labels `memory`, `session`, `turn`, `fact`; edges `relates_to`, `supersedes`, `about`,
  `contains`, `edited` (`src/catalog/schema.json`, mode `open`; `emb` is a vector rule over
  `embed_text` with the built-in `hash-ngram/1` embedder).
- Project scoping: a vertex with a `project` property (the project key: the canonical root path)
  is visible only from that project; one without it is global. Memory ids are
  `memory:<slug>@<project>` (project) and `memory:<slug>` (global); `memory`, `forget` and
  `links` look in the project first, then in the global memory.
- Sessions: `session:<transcript name>` (with `fingerprint` of its indexed lines, `summarized` =
  the fingerprint its summary came from, `harness`), `turn:<session>:<n>`, `fact:<session>:<kind>:<i>`
  (`fact_kind` decision | correction | outcome | open_thread), all with the session's `project`;
  `session -contains-> turn|fact`. They are written only by the internal catalog functions
  (`src/catalog/internal.json`: `indexed_sessions`, `index_session`, `index_turns`,
  `set_summary`), which the daemon runs on the store's session thread (`catalog::call`); they
  are never tools of the `use` server (its tool list is `FUNCTIONS`).
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
