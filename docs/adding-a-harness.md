# Adding a harness

A harness (Claude Code, opencode, Codex, Cursor, CodeWhale) is compiled in: one module that
implements the `Harness` trait, one variant of `Kind`, and its lines in the registry. Nothing
else in the code names a harness; everything shared (install, uninstall, status, doctor, the
wizard, other memory, sessions, hooks, summaries, the bench refusal, export, the command line)
asks the trait. This page is the checklist.

## 1. Verify first

Read the harness's source (or its documentation, when there is no source) for the version you
target, and cite the files in the module's docs, as the existing modules do: where it keeps its
MCP servers and in which shape, its settings and instruction files, its sessions, its hooks,
its own memory, its headless command. Behaviour you could not verify is not claimed: the
harness ships as a preview with a live-check file (`docs/<harness>-live-check.md`) until a live
run passes.

## 2. Name it

In `crates/graphersal-memory/src/harnesses/agent.rs`, add the variant to `Kind` and to
`Kind::ALL` (the wizard's order), and its line in each identity function: `id` (the
`--harness` value and the state file name), `display_name`, `program_name`, `dir_names` (the
directory names that tell an error is about it) and `cli_help` (its hand-written line under
`--harness`). Add its name to the hand-written help of `install --harness` and `uninstall
--harness` in `cli.rs`.

In `harnesses/mod.rs` (the registry), add its line to `of` (the harness built from the setup's
paths) and to `driver` (its setup-free parts: its `SessionSource` and its `Headless`, used
where there is no setup, such as the daemon's indexing and a harness command in the settings).

## 3. The module

`harnesses/<name>.rs` with `harnesses/<name>/` beside it:

| File | What it holds |
|---|---|
| `<name>.rs` | its paths (`<Name>Paths`: from the environment, and `of_home` for tests), the `<Name>` struct and its `Harness` implementation, its `HeadlessRun` when it has a headless command, its `fixture` module (tests) |
| `<name>/install.rs` | its part of install (`plan`), of uninstall (`removal`), `inspect`, `status_lines`, `doctor`, its rule block texts (`RULE_BLOCK`, `RULE_BLOCK_ANY`) and `PREVIOUS_BLOCK_FINGERPRINTS` (empty at first; every later change of a block text adds the old fingerprint) |
| `<name>/sessions.rs` | its `SessionSource`: the units (files, or a database's sessions), their id, the reader into a `Transcript` |
| `<name>/other.rs` | its `OtherMemory`: where another memory tool's MCP entries would be, its built-in memory, the instruction files it loads, how an entry is switched off and how to do it by hand |
| `<name>/hook.rs` | its hook, when it runs hooks of ours (`HookHandler`) |

### What the trait asks

Every harness gives:

- `kind`, `find_program`, `config_roots`, `has_our_parts`, `has_sessions`;
- `plan` (a `HarnessPlan`: steps, file changes, conflicts, and the state records written with
  their file), `removal`, `develop_on`, `status_lines`, `doctor`;
- its files: `mcp_config_file`, `mcp_servers_key`, `mcp_entry`, `remove_server_hint`,
  `settings_file`, `instruction_file`, `projects_dir`, `memory_dir`, `global_memory_file`,
  `has_file_memory`, `auto_memory_key`, `headless_command`;
- `user_files` (tests only, no default): its configuration as a user has it, which the
  conformance suite starts from.

What a harness may have (the default is "none"):

| Capability | When |
|---|---|
| `own_memory` (`OwnMemory`) | it has a memory of its own install can switch off (and uninstall on again); the wizard's one question covers it |
| `native_memory` (`NativeMemory`) | its own memory has a store install imports, reading it only |
| `other_memory` (`OtherMemory`) | always worth giving: other memory tools in its configuration |
| `hook` (`HookHandler`) | it runs hooks of ours; without one, `hook --harness <id>` says it needs none and its MCP connection does the session start and end |
| `own_memory_restored` | uninstall switches its own memory back on, with a report line |
| `action_needed` | the user has to do something in the harness for the install to work fully (Codex: trust the hooks) |
| `has_plugin` | it gets a session plugin rather than hooks (opencode) |
| `memory_dirs` | it has a file memory of its own (`has_file_memory`): its memory directories with their projects, which the import reads (the same format as Claude Code's: `MEMORY.md` and one file per memory); `memory_dir(root)` must be where one project's files are |
| `export_hint` | it has no file memory: how to give the agent an exported file |
| `Headless` (in `driver`) | its headless command runs the summaries (or the bench), or must be refused |

The rule for every method: it returns an outcome (a plan, lines, findings), and shared code
never asks which harness it is.

## 4. Tests

- The conformance suite (`harnesses/conformance_tests.rs`) runs every harness through the same
  checks once its registry line and `user_files` exist: install then uninstall gives every file
  back byte for byte, a second install and a refresh change nothing, a dry run writes nothing,
  status names the parts, a configuration without the program is a leftover, the develop
  server goes on and off, a read-only file is refused with nothing written, and, for a harness
  with a file memory, a memory file written in its `memory_dir` is found by its `memory_dirs`.
- The harness's own tests cover what is particular to it: the shape of its entries, its rule
  block and where it goes, its own memory switch, its session reader (with a fixture of the
  real format), its hook input.
- Tests never touch real user data: temporary homes and data directories only; a fake program
  on the search path; no network and no model call.

## 5. Documentation

- `docs/harnesses.md`: its row in the table and its section (what install changes, where its
  sessions are, what is verified and what is not).
- `README.md`: the supported harnesses and the known limits (a preview says so).
- `docs/roadmap.md`: the milestone.
- `CLAUDE.md`: its module in the layout.
- `docs/<harness>-live-check.md`: the steps of the live run that ends the preview.

## Rules that hold for every harness

- Install writes only managed, marked parts; uninstall gives every file back byte for byte, and
  a test proves it (the conformance suite does, for every harness).
- No panic on a user path; every failure is an error that says what to do.
- No network access and no model call without the user's consent.
- Paths are built with `Path::join`, one component at a time, on every platform.
