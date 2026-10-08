# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Rust rewrite (ratatui + async MongoDB driver + tokio) of the Python/Textual app in the parent directory. `../CLAUDE.md` describes that Python app, not this crate. The rewrite has to stay compatible with it: CLI options and env vars, key bindings, theme names and colors, and the config file at `~/.config/close-mongo-ops-manager/config.json` (or `$XDG_CONFIG_HOME/...` when absolute). `README.md` lists the options and keys, and documents what changed from the Python version.

## Commands

Run everything from `rust/`. `rust-toolchain.toml` pins the toolchain (1.99.0); the MSRV is 1.88 (edition 2024).

```shell
cargo build
cargo test                                    # unit tests (integration tests skip themselves without env vars)
cargo test <name_substring>                   # e.g. cargo test mongo::kill::tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings     # CI also passes --locked
cargo run -- --host 127.0.0.1                 # or --uri 'mongodb://...' [--all-nodes]
make lint | make test | make dist             # dist = scripts/build-release.sh (see --help)
```

CI (`../.github/workflows/rust.yml`) runs fmt, clippy with `-D warnings`, `cargo test --locked`, and `shellcheck scripts/*.sh`. Pushing a `rust-v<version>` tag builds the release binaries and fails if the tag doesn't match the version in `Cargo.toml`.

### Integration tests (real MongoDB cluster in Docker)

```shell
eval "$(tests/docker/cluster.sh up)"          # 2 mongos, configRS, shard01/shard02, standalone (ports 37017-37031)
cargo test --test mongo_integration
cargo test --test mongo_integration <name>
tests/docker/cluster.sh down                  # also: status, env, stop-member/start-member PORT
```

Each test returns early and passes when its `CMOM_IT_*` variable is unset, so a green `cargo test` run without the cluster tells you nothing about the MongoDB layer. The tests share one cluster and serialize themselves through a mutex.

## Architecture

Data flows one way: **runtime → App (pure state machine) → Effects → runtime workers → AppEvents → App**.

- `cli.rs` parses options with clap, initializes logging, loads the theme from the config store, builds a multi-thread tokio runtime and calls `runtime::run`.
- `runtime.rs` sets up the terminal (raw mode, alternate screen, mouse, panic hook that restores the terminal) and runs the event loop. It merges terminal input, a 100 ms `Tick` and worker results into `AppEvent`s, and carries out the returned `Effect`s in `Workers`: connect, fetch, kill (up to 8 at once), save theme. Workers wrap background futures in `catch_panic`, so a panic becomes an error the app receives as an answer instead of a dead UI. A newer fetch aborts the one in flight, and a stale connect attempt is shut down.
- `app/` holds the whole UI state. `App::update(event, now) -> Vec<Effect>` does no I/O and takes the clock as a parameter. Auto-refresh, the 250 ms filter debounce, the loading indicator and toast expiry all run off `Tick` plus `now`. Fetches carry a `generation`, and results from superseded fetches are ignored. `Action` and `HELP` define the footer and help text; update the README's Keys section when you change them.
- `ui/` renders `&mut App` through ratatui. While drawing, it records clickable rectangles in `app.layout` (`LayoutCache`), which `app/update.rs` uses to map mouse clicks, and it clamps modal scroll offsets to their content. The App only *requests* scroll positions.
- `mongo/` holds `MongoManager`, which is shared as `Arc` between tasks:
  - `call.rs`: every server round trip is bounded by `--timeout` (with a short grace period), and aggregations also set `maxTimeMS`. Our own `$currentOp` calls carry a random per-process `comment` marker so the listing can hide them. Operations are never hidden by appName.
  - `pipeline.rs`: builds the `$currentOp` pipelines. Filters become `$match` stages that run on the server. The pipeline sorts by `microsecs_running` desc and keeps `MAX_OPERATIONS` (1000) + 1 to detect truncation.
  - `parse.rs`: converts the documents into `model::Operation`, merges per-node listings, and identifies each operation by `OpKey`.
  - `topology.rs`: identifies the deployment through `hello`, so connecting needs no `serverStatus` privilege.
  - `cluster.rs`: implements `--all-nodes`. `NodePool` discovers shard and config server members (rediscovering every 30 s), polls each member over its own direct client, reports member health, and falls back to listing a shard through mongos when its primary can't be polled.
  - `kill.rs`: kills safely (see below).
- `model.rs` holds the types shared across layers (`Operation`, `OpId`, `OpKey`, `OpSource`, `Snapshot`, `KillRequest`/`KillOutcome`, `ServerInfo`, `NodeStatus`).

### Invariants to preserve

- **Kill safety.** Opids are per-server counters and get reused. A kill always goes to the server that listed the operation (`OpSource` tells which one; `plan_kill` decides the route). Before the kill, a lookup checks that the opid still belongs to the same operation (same start time, or same `desc`/`connectionId`), and the result is verified afterwards. On mongos, never strip the `shard:` prefix from an opid, because a bare number addresses one of mongos' own operations.
- **Cancellation safety.** A `MongoManager::fetch` future can be dropped at any `await`, so shared state may change only in synchronous steps. Compile-time asserts in `mongo/mod.rs` require `MongoManager` to be `Send + Sync` and its public futures to be `Send`.
- `OpKey` must stay stable across refreshes, because selection and the cursor are preserved by key.

## Testing conventions

- Unit tests live next to the code (`#[cfg(test)] mod tests`). The large suites are `app/tests.rs`, which uses a `Harness` with a controllable `Instant` to drive `App::update`, and `ui/tests.rs`, which renders to ratatui's `TestBackend` and asserts on buffer cells and styles. Shared fixtures (`sample_operation`, `operation_running`) are in `src/testutil.rs`.
- Every source file starts with a `//!` module doc, and most items have `///` docs. Keep that up.

## Release builds

`scripts/build-release.sh` builds `aarch64-apple-darwin` natively (macOS only) and the two `*-unknown-linux-musl` targets as fully static binaries in Docker (`docker/build/Dockerfile`, which cross-compiles with clang and rust-lld instead of QEMU). It writes `dist/*.tar.gz` and `SHA256SUMS`. The script has to stay compatible with macOS bash 3.2 and pass shellcheck. TLS uses rustls, with no OpenSSL dependency.
