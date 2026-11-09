# CLAUDE.md

Guidance for AI agents working in this repository. When this file disagrees with the code, the code
wins: say so, so the stale section gets fixed.

**graphersal-memory** is a local graph memory for AI coding agents, a one-click product built on
the [Graphersal](https://github.com/graphersal/graphersal) engine. It is in pre-release: the user-facing
design is in `README.md` and `docs/`, and the stage plan is in `docs/roadmap.md`.

## Layout

- `crates/graphersal-memory/`: the single binary crate (`graphersal-memory`). There is one crate
  until a second binary or a library user appears, with modules per area:
  - `src/main.rs`: the entry point (parse, run, exit code);
  - `src/cli.rs`: the clap command definitions and help text, plus parsing tests;
  - `src/commands.rs`: running a command (`run_cli`): `mcp` runs the shim (`--no-daemon`:
    serves in this process), `daemon` runs (`--foreground`) or starts the daemon, `install`,
    `uninstall`, `status`, `doctor`, `import` and the hidden `hook` work on Claude Code (`Setup`
    from the environment); `ui`, `bench`, `export` report their planned stage and exit with code 2;
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
    changed memory files, one status line at session start, always exit 0;
  - `src/config.rs`: `<data dir>/config.json` (`daemon.idle`); flag > env > file > default;
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
    handshake to a `StoreRuntime` (opened on first use) and its profile, idle exit, drain/restart
    for a newer shim, clean shutdown (socket first, then the stores);
  - `src/registry.rs`: `registry.json` (project -> store; everything goes to `default` until
    isolation, M7);
  - `src/engine.rs`: the pinned engine revision, and a test that exercises the engine crates;
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
    profile over it sharing one `Mcp` endpoint, the job ticker, `serve` of one connection through
    `graphersal-mcp`'s `serve_lines`, `close`), `serve` (`--no-daemon`) and `ProjectBinding` (sets the `project` argument of the memory functions on
    every `tools/call` and hides it in `tools/list`, on the JSON-RPC lines);
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
- Planned areas (one module each when they arrive): `sessions`, `bench`, `export`, other
  harnesses (`install.rs` is Claude Code only; split it per harness when the second arrives).

## Memory model

- Labels `memory`, `session`, `turn`, `fact`; edges `relates_to`, `supersedes`, `about`,
  `contains`, `edited` (`src/catalog/schema.json`, mode `open`; `emb` is a vector rule over
  `embed_text` with the built-in `hash-ngram/1` embedder).
- Project scoping: a vertex with a `project` property (the project key: the canonical root path)
  is visible only from that project; one without it is global. Memory ids are
  `memory:<slug>@<project>` (project) and `memory:<slug>` (global); `memory`, `forget` and
  `links` look in the project first, then in the global memory.
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

## Engine dependency

- `graphersal` and `graphersal-mcp` are git dependencies pinned to one revision of graphersal
  `main` in the root `Cargo.toml`. During 0.x they become exact `=0.6.x` crates.io pins once
  graphersal 0.6.0 is published. Every engine crate (`graphersal`, `graphersal-mcp`,
  `graphersal-serve`, `graphersal-session`) pins the same revision; when you move the pin, update
  `engine_rev_short!` in `src/engine.rs` (a test checks that they match).
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
3. **The user's live configuration is never changed by a benchmark**, and install only writes
   managed, marked parts. `uninstall` must restore the configuration byte for byte, and a test
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
