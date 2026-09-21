# CLAUDE.md

Guidance for Claude Code (claude.ai/code) working in this repository.

## Project Overview

`rust-analyzer-mcp` is an MCP server fronting `rust-analyzer` over LSP, so that an assistant can
ask about Rust code by symbol rather than by text. This is the `IconRunner05` fork of
`zeenix/rust-analyzer-mcp`; `upstream` is the original and prefer upstreaming a fix over carrying
it.

`--version` says which build is installed — the fork and the crates.io crate share a `version`, so
the revision and tool count are what tell them apart:

```
rust-analyzer-mcp 0.4.0 (fork, rev <short sha>, 21 tools)
```

## Architecture

Four layers, each a directory, with the protocol types they speak kept apart from the code that
speaks them:

- **`src/main.rs`** — 49 lines. Parses the command line, builds the server, runs it on a Tokio
  runtime, and shuts that runtime down in the background because Tokio's stdin reader parks an
  uncancellable blocking read that would otherwise hang the process on a signal.
- **`src/cli.rs`** — hand-rolled argument parsing (no clap) into an `Action`. Takes `OsString`s so
  a workspace path that is not valid UTF-8 is passed through rather than rejected.
- **`src/mcp/`** — the MCP half. `server.rs` owns the request loop and the map of workspace root →
  client; `tools.rs` is the tool definitions and their annotations; `handlers.rs` is one function
  per tool.
- **`src/lsp/`** — the rust-analyzer half. `connection.rs` is the subprocess and framing,
  `client.rs` the initialize sequence and document state, `handlers.rs` one method per LSP request.
- **`src/protocol/`** — the wire types for both, `mcp.rs` and `lsp.rs`.
- **Leaf modules** — `uri.rs` (path ↔ URI, including Windows spellings), `position.rs`,
  `locations.rs` (turning LSP `Location`s into readable hits), `blast.rs` (the test/production
  partition), `diagnostics/`, `settings.rs`, `config.rs`.
- **`build.rs`** — records the git revision into the binary, which is what makes `--version`
  able to answer.

### Decisions worth knowing before changing anything

- **One `rust-analyzer` per workspace root**, cached in `server.clients`. A call may carry
  `workspace_path` and is then answered by that workspace's own client *for that call only*, with
  the default left where it is — so concurrent callers can work in different repos. Only
  `set_workspace` moves the default, and doing so respawns `rust-analyzer` and reindexes.
- **Reads are gated on the index** (`ensure_index_ready`). Everything worked out from the whole
  workspace answers `null` or `[]` until that workspace is loaded, and those are the same answers
  given for a symbol nothing refers to and a position that is not on a symbol. The gate turns the
  wait into an error that says so. `symbols` is deliberately ungated — it is syntactic and answers
  from the one file — but retries behind the gate if the answer is `null`.
- **An empty answer is explained** (`explain_empty_answer`) when the loaded workspace accounts for
  it: a file outside the root, or a root with no `Cargo.toml`.
- **A runnable's range is the code it would run, except a doctest's.** `blast.rs` splits hits on
  rust-analyzer's test runnables, and a `doctest` runnable reports the range of the *item the doc
  comment is attached to* — signature and body — not of the fenced lines. Trusting it files every
  reference inside a documented function under `tests`. Nothing is lost by dropping it: doctest
  code is a string in a comment and no reference edge resolves into it, so a doctest span can
  never hold a hit that is its own.
- **Coordinates are 0-based**, and positions are on the identifier. `symbols` returns hierarchical
  `DocumentSymbol`s, so `selectionRange.start` is the identifier and `range.start` is the doc
  comment.
- **Nothing here writes to a file.** `format`, `rename` and `ssr` all work out an edit and hand it
  back; applying it is the caller's decision. That is why all three are annotated `readOnlyHint`.
- **A tool description is read by a model, not by a person browsing docs.** The whole listing goes
  out at the start of every session holding this server, so a sentence in it is paid for per seat
  rather than per repo. Write what changes what the caller does -- the shape of the answer, and
  the traps that make a wrong answer look like a right one -- and leave the rationale to the doc
  comment on the handler, which costs nothing to ship. The shared parameter text lives in
  `WORKSPACE`/`FILE`/`LINE`/`CHARACTER` and the shared schemas in `at_position()`/`in_file()`,
  because the same paragraph repeated across twenty tools was two thirds of the listing.

## Development Commands

```bash
cargo build                      # development build
cargo run -- /path/to/workspace  # run against a workspace
cargo run -- --version           # which build is this
cargo build --release            # optimized, LTO
RUST_LOG=debug cargo run -- /path/to/workspace

cargo test                       # everything
cargo test --lib blast           # one module's unit tests
cargo test --test integration_tests ssr
cargo test -- --nocapture

cargo +nightly fmt               # format
cargo clippy --all-targets -- -D warnings   # the CI gate
```

## Testing

- `tests/integration/` — one file per area, against a real server over a Unix socket
  (`test-support`, `IpcClient::get_or_create`, which shares one server between tests).
- `tests/unit/`, `tests/property/`, `tests/stress/` — protocol shapes, fuzzing, concurrency.
- `test-project/` and `test-project-diagnostics/` — fixtures. They are `exclude`d from the
  workspace so their faults are not the workspace's faults.

**Every fixture and assertion must be able to fail for the reason it exists.** Ask of a check what
it would print if the thing it guards were completely broken; if the answer is "it passes", the
check is decoration. Two measured examples from this repo:

- A diagnostics fixture with **one** faulted file cannot tell "the file you opened" from "the whole
  workspace", so it matched `cargo` while the tool reported the wrong set entirely. A workspace
  test needs at least two faulted files in two files.
- `test-project/src/blast_fixture.rs` exists because a test/production split is only falsifiable
  when production code and its `#[cfg(test)]` tests share a file. A split by filename reports 4
  production callers and 0 tests there; the real one reports 2 and 2. One of the two production
  callers carries a doctest, so the fixture also discriminates a rule that is wrong the other
  way — trusting a doctest runnable's range reports 1 and 3 — and no count it produces is
  reachable by either wrong rule.

Confirm a new check by deliberately breaking what it guards and watching it go red. Every number
matching on the first try is a reason to test whether the fixture discriminates, not evidence that
it does.

### CI

Tests detect CI via `std::env::var("CI")` and adjust: concurrent tests run in smaller batches,
tool-call timeouts go from 10s to 30s, and `test_rapid_fire_requests` spaces out its spawns. When a
CI failure does not reproduce locally, look there first.

## Git Commit Conventions

Use gitmoji. Picker: https://zeenix.github.io/gimoji/ · raw: https://zeenix.github.io/gimoji/emojis.json

Write the message about the problem, not the patch: what went wrong, what it looked like when it
did, and why this is the fix. The measurement that established a bug belongs in the commit that
fixes it.
