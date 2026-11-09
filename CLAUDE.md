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
  - `src/commands.rs`: running a command (`run_cli`); `mcp --no-daemon` serves, every other
    command reports its planned stage and exits with code 2;
  - `src/engine.rs`: the pinned engine revision, and a test that exercises the engine crates;
  - `src/store.rs`: the data directory (`--data-dir`, `GRAPHERSAL_MEMORY_DIR`, the platform's
    per-user directory) and the default store `<data dir>/stores/default`, created on first use;
  - `src/catalog.rs` + `src/catalog/`: the product catalog built into the binary: `schema.json`,
    `queries.json` (name, description, parameters of each memory function) and one
    `<name>.rhai` body per function (folder `memory`, `meta` `{"origin": "product", "catalog":
    <fingerprint>}`); `apply` writes it to a store whose product queries carry another
    fingerprint; `use_policy` builds the `use` access policy from the functions' inferred effects;
  - `src/mcp.rs` + `src/mcp/{use,develop}.md`: the two profiles (configuration, instructions,
    tool lists), the project of a session (`--project`, else the git root of the cwd), `serve`
    (one store over stdio through `graphersal-serve`'s session thread and `graphersal-mcp`'s
    `serve_lines`) and `ProjectBinding` (sets the `project` argument of the memory functions on
    every `tools/call` and hides it in `tools/list`, on the JSON-RPC lines);
  - `src/mcp_tests.rs`: MCP round trips over in-memory pipes with temporary data directories.
- Planned areas (one module each when they arrive): `daemon` + `shim`, a store registry,
  `install/<harness>`, `sessions`, `bench`, `export`/`import`.

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
   and data directories. It NEVER reads or writes the real `~/.claude`, the user's stores or
   transcripts, or a running daemon or MCP server of the user. Tests make no network calls and
   call no model; LLM-dependent paths are tested with a fake harness binary.
3. **The user's live configuration is never changed by a benchmark**, and install only writes
   managed, marked parts. `uninstall` must restore the configuration byte for byte, and a test
   proves it.
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
