# close-mongo-ops-manager (Rust)

Monitor and kill MongoDB operations from the terminal. A single self-contained
binary, built with the official [MongoDB Rust driver](https://github.com/mongodb/mongo-rust-driver)
(async) and [ratatui](https://github.com/ratatui/ratatui).

This is a rewrite of the Python/Textual application in the parent directory. It
keeps its options, key bindings, themes and configuration file, and adds better
support for sharded clusters.

## What's new compared to the Python version

- **Connection strings**: `--uri` / `MONGODB_URI` accepts any `mongodb://` or
  `mongodb+srv://` URI, so TLS (`tls=true&tlsCAFile=...`), x.509, SRV, multiple
  mongos seeds and other driver options work. `--host` also takes a
  comma-separated seed list.
- **Every cluster member, PMM-style** (`--all-nodes`): discovers shard and
  config server replica set members (or the members of a replica set) and polls
  each one over its own direct connection, so operations on secondaries and
  config servers are listed too. A member that can't be reached doesn't block
  the refresh: its status shows in the status bar and in the Nodes dialog
  (`Ctrl+N`). When a shard primary can't be polled directly, that shard's
  operations are listed through mongos instead.
- **Topology columns**: Shard and Node (with the member's role) columns when
  connected to mongos or with `--all-nodes`.
- **mongos' own operations** (`Ctrl+O`): also list the operations running on
  the connected mongos (`$currentOp` with `localOps: true`).
- **Safer kills**: opids are per-server counters and get reused. Each kill is
  sent to the server that reported the operation, and only after checking that
  the opid still belongs to the same operation (same start time and host). The
  Python fallback that retried a `shard:opid` kill with only its number through
  mongos (which addresses a different operation, on mongos itself) is gone.
- **Bounded waits**: every server round trip has a timeout (`--timeout`,
  default 5 s), so a slow or unreachable shard never freezes the screen.
- **Our own operations are hidden**: the app tags its own `$currentOp` calls
  with a random per-process marker and hides them (with the system
  operations). It still connects with the `close-mongo-ops-manager`
  application name, but never hides operations by application name, which any
  client can choose.
- The deployment is detected with `hello` (no `serverStatus` privilege needed
  to connect).

## Install

Download the binary for your platform from the releases (or build it, see
below) and put it on your `PATH`:

| Platform | Target |
| --- | --- |
| macOS, Apple Silicon | `aarch64-apple-darwin` |
| Linux, arm64 | `aarch64-unknown-linux-musl` (static) |
| Linux, x86-64 | `x86_64-unknown-linux-musl` (static) |

The Linux binaries are statically linked and run on any distribution. TLS uses
rustls; there is no OpenSSL dependency.

## Usage

```shell
close-mongo-ops-manager --help

# Local server
close-mongo-ops-manager

# Host, port and credentials, as in the Python version
close-mongo-ops-manager --host db.example.com --port 27017 --username admin --password secret

# Any connection string
close-mongo-ops-manager --uri 'mongodb+srv://user:pass@cluster0.example.mongodb.net/?tls=true'

# Sharded cluster: poll every shard and config server member directly
close-mongo-ops-manager --uri mongodb://mongos1:27017,mongos2:27017 --all-nodes
```

### Command line options

| Option | Environment | Default | Description |
| --- | --- | --- | --- |
| `--uri` | `MONGODB_URI` | | Connection string; takes precedence over `--host`/`--port` |
| `--host` | `MONGODB_HOST` | `127.0.0.1` | Host, or comma-separated `host[:port]` seed list |
| `--port` | `MONGODB_PORT` | `27017` | Port for hosts given without one |
| `--username` | `MONGODB_USERNAME` | | Username |
| `--password` | `MONGODB_PASSWORD` | | Password |
| `--auth-source` | `MONGODB_AUTH_SOURCE` | `admin` | Authentication database |
| `--namespace` | | | Only list operations whose namespace starts with this |
| `--refresh-interval` | `MONGODB_REFRESH_INTERVAL` | `2` | Seconds between refreshes (1–10) |
| `--show-system-ops` | | off | Show system operations |
| `--load-balanced` | | off | Connect through a load balancer |
| `--all-nodes` | `MONGODB_ALL_NODES` | off | Poll every cluster member directly |
| `--node-username` | `MONGODB_NODE_USERNAME` | `--username` | Username for direct member connections |
| `--node-password` | `MONGODB_NODE_PASSWORD` | | Password for `--node-username` |
| `--node-auth-source` | `MONGODB_NODE_AUTH_SOURCE` | `--auth-source` | Auth database for `--node-username` |
| `--timeout` | `MONGODB_TIMEOUT` | `5` | Seconds allowed for connecting and each round trip |
| `--log-file` | `CLOSE_MONGO_OPS_MANAGER_LOG_FILE` | `close_mongo_ops_manager.log` | Log file (replaced by a new owner-only file at startup) |
| `-v`, `--verbose` | | off | Debug logging |
| `--version`, `--help` | | | |

With `--all-nodes` on a sharded cluster, keep in mind that users created
through mongos live on the config servers: direct connections to shard members
need shard-local users. Create them on each shard's primary, or pass
`--node-username`/`--node-password`.

### Keys

```
F1, ?           Show help
Ctrl+Q, Ctrl+C  Quit
Ctrl+R          Refresh (reconnect after a connection failure)
Ctrl+K          Kill selected operations
Ctrl+P          Pause/Resume auto-refresh
Ctrl+S          Sort by running time
Ctrl+L          View application logs
Ctrl+A          Toggle selection (select all/deselect all)
Ctrl+F, /       Toggle the filter bar
Ctrl+T          Change theme
Ctrl+O          Show/hide the operations of mongos itself
Ctrl+N          Cluster members and their status (--all-nodes)
+, Ctrl++       Increase refresh interval
-, Ctrl+-       Decrease refresh interval
Enter           Operation details
Space           Select operation
Tab, Shift+Tab  Move between the table and the filters
Esc             Close dialogs, leave the filter bar
```

The mouse works too: click a row to select it, click the footer entries,
the filter inputs, the Clear button and the dialog buttons, and use the wheel
to scroll. Selections survive refreshes while the operations are still
running. Pause auto-refresh (`Ctrl+P`) for a stable view while deciding what to
kill.

Filters match case-insensitively anywhere in the field; "Running Time ≥ sec"
takes a number of seconds. The filter bar keeps Textual's line editing keys
while typing (`Ctrl+A`/`Ctrl+E` start/end, `Ctrl+U`/`Ctrl+K` delete to
start/end, `Ctrl+W` delete word).

### Themes

`textual-dark` (default), `textual-light`, `nord`, `gruvbox`, `tokyo-night`,
`solarized-light`, `dracula`, `monokai`, `flexoki`, `catppuccin-mocha`,
`catppuccin-latte` and `close-mongodb` (MongoDB brand colors). The choice is
saved to `~/.config/close-mongo-ops-manager/config.json`, the same file the
Python version uses (when `$XDG_CONFIG_HOME` is set to an absolute path, the
file is under it instead). On terminals
without true color support, colors are mapped to the 256-color palette.

### Logs

The log goes to `close_mongo_ops_manager.log` in the working directory (as in
the Python version; change it with `--log-file`) and can be viewed in the app
with `Ctrl+L`.

## Development

Requires Rust 1.88 or newer (`rust-toolchain.toml` pins the version used for
releases).

```shell
cd rust
cargo test                                   # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run -- --host 127.0.0.1                # run the app
```

### Layout

| Path | Purpose |
| --- | --- |
| `src/main.rs`, `src/cli.rs` | Entry point, command line options |
| `src/runtime.rs` | Terminal setup, event loop, background tasks |
| `src/app/` | Application state machine (`App::update` turns events into effects; no I/O) |
| `src/ui/` | Rendering with ratatui |
| `src/mongo/` | Connecting, topology discovery, `$currentOp`, killing |
| `src/model.rs` | Types shared by the layers |
| `src/theme.rs`, `src/config.rs`, `src/logging.rs` | Themes, configuration file, log |

# Interface
![App screenshot](img/close-mongo-ops-manager.png "Close Mongo Ops Manager")

