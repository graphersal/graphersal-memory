# Contributing

Thank you for your interest. graphersal-memory is young and changes quickly, so please open an
[issue](https://github.com/graphersal/graphersal-memory/issues) before starting on a larger
change: the [roadmap](docs/roadmap.md) may already cover it, or it may conflict with work in
progress. Bug reports with `graphersal-memory --version`, your platform, your harness and the
output of `graphersal-memory doctor` are the most useful.

## Before you open a pull request

Building needs Rust 1.88 or newer and a C compiler. The browser UI's tests run with Node.js:
`node --test crates/graphersal-memory/ui/tests/*.test.mjs`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- **Tests never touch your real configuration or data.** Use temporary directories for `HOME`,
  the harness configuration and the data directory. A test that reads or writes `~/.claude`, a
  real store or a real transcript is a bug.
- **No panics on user paths.** Return errors with a message that says what to do.
- **English** in code, comments and docs.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `docs:`, `test:`, `chore:`).
- Engine problems, such as missing APIs or bugs in queries, storage or MCP, belong in
  [Graphersal](https://github.com/graphersal/graphersal/issues), not in a local workaround.

By contributing, you agree that your contributions are licensed under the Apache License,
Version 2.0 (see [LICENSE](LICENSE)).
