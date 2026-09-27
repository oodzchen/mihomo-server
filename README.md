# Mihomo Server

A headless Mihomo management service based on the extraction plan in
[headless.md](headless.md).

See [architecture and migration status](docs/ARCHITECTURE.md) for the complete
target architecture, current completion states, and the usable MVP boundary.

The workspace includes upstream draft transactions, rate limiting, subscription
schemas, and pure configuration field, merge, and sequence operations, exposed
by `headless-core`, plus a standalone Mihomo API client. The service now runs
continuously and manages one Mihomo child, including readiness, restart, bounded
recovery, state/log subscriptions, and Unix shutdown. See
[running instructions](docs/RUNNING.md) for the executable core-management workflow.
Runtime configurations now use immutable revisions and an atomic commit record.
Imports and overlays pass YAML and `mihomo -t` validation before application;
hot reload has a restart fallback, failed applications attempt rollback, and
service restarts restore the committed configuration and active profile. Local
subscription imports, catalog persistence, selection, and profile snapshots are
connected using the upstream file schema. Node selections are saved per profile,
restored after startup/reload, and repaired with the upstream reconciliation
logic. Restoration retries incomplete groups with a deadline and is cancelled
by newer manual selections and lifecycle operations. An authenticated Axum API
now exposes lifecycle, YAML/profile uploads, configuration editing, node selection,
and state/log/profile/config/proxy queries, including when the core is stopped or
failed. Authenticated WebSockets now provide state/profile/log events and forward
traffic, memory, connections/count and core logs, with per-session cancellation
and core reconnects. A minimal responsive React UI now connects these workflows
and is served by Rust from built assets. Direct remote URL import now preserves
upstream metadata and raw YAML; activation remains separate. Manual remote refresh
preserves UID/node records and validates/applies active updates with recovery. Metadata
edits and protected noncurrent deletion preserve runtime state. Linked YAML merge
enhancements now persist independently of raw subscriptions and regenerate on
selection/active refresh, with validated edits, detach and recovery. Linked rules,
proxies and groups sequences now use the same transaction and upstream generation
order before merge.
Linked JavaScript main(config, name) now runs after merge in a cancellable Linux
worker with resource limits, failure diagnostics and transactional persistence.
Scheduled refresh and managed/system/direct proxy download routes are supported.
The `/core` page now upgrades or repairs the managed core through stable and Alpha
channels, with force/no-op, durable rollback and installation receipt readback.
An authenticated `/api/backup` endpoint exports bounded service ZIP snapshots.
`/api/backup/inspect` checks uploaded snapshots without extracting or applying them.
`/api/backup/validate` rehearses runtime/enhancement restoration with isolated probes.
The full settings/resource pipeline, backup restore/WebDAV, advanced pages and other
platforms remain pending.

## Layout

```text
crates/clash-verge-draft/ Upstream draft state and transaction library
crates/clash-verge-limiter/ Upstream rate limiter
crates/mihomo-client/   Tauri-free API client and realtime subscriptions
crates/headless-core/  Configuration and subscription business logic
service/               Single service entry point, HTTP/WS, process management
web/                   React browser UI, HTTP/WS adapters and real-browser test
scripts/               Explicitly pinned local Linux bundle preparation
deploy/                Foreground launcher and optional systemd template
docs/UPSTREAM.md        Pinned source revisions and extraction provenance
docs/DEPLOYMENT.md      Verified bundle deployment and persistent-core boundary
```

The service uses the core library and standalone Mihomo client. The core library
remains independent of Tauri and HTTP frameworks.

## Run with the browser UI

Build from the repository root, then launch one Rust service with an existing core:

```sh
npm --prefix web ci
npm --prefix web run build
cargo build --workspace --locked
target/debug/mihomo-server --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data --web-dir ./web/dist --config ./examples/minimal.yaml
```

Open `http://127.0.0.1:9090` and enter the token from `./data/management-token`.
The token is kept in browser memory; refreshing requires login again. To save
node choices, import and select a local YAML profile in the browser first.
Omit `--web-dir` for API-only operation. Node is needed only to build/test assets.
See [browser instructions](docs/RUNNING.md#browser-ui) and [frontend scope](web/README.md).
The initial Linux local-profile MVP is verified. For a complete local bundle with
a pinned independent core and persistent data/core locations, use
[deployment instructions](docs/DEPLOYMENT.md). Other platforms and full features remain pending.

## Development

The workspace uses edition 2024 and pins Rust 1.98.1, `rustfmt`, and `clippy` in
`rust-toolchain.toml`, matching the upstream checkout. Cargo uses this toolchain
automatically. Dependency versions are recorded in `Cargo.lock`; download them
on the first build before using offline commands.

```sh
cargo build --workspace --locked
cargo test --workspace --locked --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo run --locked --offline -p mihomo-server -- --help
```

The existing draft tests cover commit/discard, serialized asynchronous updates,
conflict detection, transaction commit/rollback, and exclusive layer claims.
Draft transactions cover in-memory layers; runtime storage and the lifecycle actor
provide separate disk/configuration recovery.
The limiter, merge, sequence, and field ordering tests are reused from upstream.
An extraction-specific schema test protects subscription metadata and persisted
node selections during YAML serialization.

The client retains upstream API methods and has real-core integration tests for
Linux loopback HTTP and Unix sockets. See [client usage and validation](crates/mihomo-client/README.md).
The manager is verified against the real core for startup, failed startup and
manual retry, reload, restart, crash recovery, configuration persistence/recovery,
and SIGTERM/SIGINT child cleanup. Controlled processes also verify reload/restart
failure rollback, validation timeout/cancellation, and bounded output capture.
Local subscription import/selection tests cover failed switches and interrupted
current-profile updates, including actual CLI import and service restoration.
Node tests verify persistence without Mihomo's own selection cache, profile
isolation, automatic-group unfix, failed/unconfirmed selections, disk-write rollback,
delayed groups, manual supersession, restoration deadlines, and shutdown cancellation.
HTTP tests cover authentication, request limits and failed-core repair; a real
service test verifies API import, selection, reload, restart/restoration and cleanup.
WebSocket tests cover authentication before data, bounded admission, reconnect
snapshots, independent subscriptions, core restart and shutdown cleanup.
Static asset tests verify path restrictions and API separation. A real Chromium
test verifies the browser workflow against the built Rust service and core,
including restart restoration and mobile layout; see [frontend validation](web/README.md).
Resource and real bundle tests verify first-use checksum checks, persistent core
preservation, relocated launch, repair/restoration and SIGTERM child reaping.
The release bundle passes the same Chromium browser workflow. Remote tests cover
metadata, bounded downloads, failure preservation, shutdown and real-core restoration
without refetching. Manual refresh tests cover active/noncurrent/stopped profiles,
stale responses, interrupted commits and application failure recovery. Metadata/delete
tests cover strict patches, current protection, interrupted cleanup, concurrent refresh
and restart persistence. Linked merge tests cover upstream semantics, interrupted
commits, validation/application failure rollback, raw/node retention, refresh and
browser restart/detach. Sequence tests cover ordering, reference cleanup, shared
links, old journal compatibility, stale downloads, failure rollback and browser
type switching/node restoration. Script tests cover source/raw retention, failure
diagnostics, strict commands, time/memory/output limits, worker reaping and browser
restart/detach. Next implement global Merge/Script defaults and execution ordering,
followed by remaining enhancement and scheduled workflows.

## License

The project uses GPL-3.0-only; see [LICENSE](LICENSE). The extracted Mihomo
plugin has no upstream license declaration and does not inherit that license.
See [docs/UPSTREAM.md](docs/UPSTREAM.md) for provenance and the unresolved plugin
licensing status.
