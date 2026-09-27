# Mihomo Client

Tauri-free extraction of `tauri-plugin-mihomo` commit
`ba8434c08c869916b8041d707ad66599bb5230b2`. Requests, response models, API
methods, error handling, local-socket transport, and WebSocket reader
cancellation are retained. See [upstream provenance](../../docs/UPSTREAM.md).

The pinned upstream repository does not declare a license in its source tree,
Cargo manifest, package manifest, or README. This crate therefore does not
inherit the project's GPL declaration. Resolve the upstream license before
publishing or redistributing this extracted code.

## Usage

Create a client inside a Tokio runtime. It does not start or own a Mihomo process.

```rust,no_run
use mihomo_client::{Builder, models::Protocol};

# async fn example() -> mihomo_client::Result<()> {
let client = Builder::new()
    .protocol(Protocol::LocalSocket)
    .socket_path("/path/to/core.sock")
    .build()?;
let version = client.get_version().await?;
# Ok(())
# }
```

For loopback HTTP, set `external_host`, `external_port`, and `secret` explicitly.
Ordinary requests have a five-second timeout; reload, downloads, and provider
updates retain their upstream operation-specific timeouts. A builder does not
automatically launch background debug tasks.

`ws_traffic`, `ws_memory`, `ws_connections`, and `ws_logs` deliver JSON bytes
through callbacks. `ws_connections_count` delivers a count. The corresponding
public `*_checked` methods deliver `WsMessage` and accept a callback returning
`false` when its consumer is gone; the reader then releases the connection.
Plain error text is delivered as a JSON string, preserving upstream framing.
An empty connection snapshot with `connections: null` produces a count of zero.

Disconnect individual subscriptions with `disconnect(id, Some(0))`, or call
`clear_all_ws_connections()` during shutdown. Reconnect by establishing a new
subscription. Keep callbacks brief; they run on the reader task. If the optional
debug connection watcher is started, retain and abort its returned task handle
when it is no longer needed.

The library retains upstream controller restart and upgrade API methods for API
compatibility. The management service must use its own lifecycle manager and
managed binary upgrade workflow as described in `headless.md`.

## Validation

```sh
cargo test -p mihomo-client --locked --offline
MIHOMO_TEST_BINARY=/absolute/path/to/mihomo cargo test -p mihomo-client \
  --test live_core --locked --offline -- --ignored --test-threads=1
```

The default suite reuses upstream model and WebSocket payload tests and adds
the null-connection regression. Live tests are explicit because they require a
real core and local socket binding permissions. They create isolated temporary
data, disable proxy listeners, use an ephemeral loopback controller or a private
Unix socket, and terminate and wait for the owned child before cleanup. They
exercise queries, encoded group names, node selection, HTTP authentication,
configuration patch and reload, all realtime feeds, disconnect, receiver drop,
reconnection, and bulk subscription cancellation.

Linux HTTP and Unix socket behavior is verified against the locally available
Mihomo v1.19.31. Windows Named Pipe code is retained but has not been validated
on Windows. Provider fetches, remote delay checks, Geo downloads, and upgrade
operations are retained but not exercised by these local integration tests.
