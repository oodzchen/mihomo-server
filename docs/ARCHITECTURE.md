# Architecture and migration status

This document tracks the implementation of [headless.md](../headless.md).
Update it after each migration and include the complete architecture with
completion states in the accompanying progress report.

Status meanings:

- **Migrated**: upstream implementation is extracted and relevant behavior is verified.
- **Implemented**: a new service adaptation works and is verified.
- **Scaffold**: the package or directory exists, but its runtime behavior is incomplete.
- **Partially migrated**: some named responsibilities are verified; the remaining ones are pending.
- **Partially implemented**: service runtime pieces work, while the remaining integrations are pending.
- **Pending**: the component has not been implemented or extracted.

A copied source file, a successful build, or an empty route does not by itself
establish a working integration. Mark partially migrated components explicitly.

## Complete target architecture

This tree describes target responsibilities. Pending entries are planned
components, not directories or modules that already exist. Extract additional
crates only when their dependencies and actual use justify the split.

```text
mihomo-server/
├── Cargo workspace / Rust 1.98.1 / edition 2024       [Implemented]
├── crates/
│   ├── clash-verge-draft/                            [Migrated]
│   │   └── Snapshots, drafts, transactions, tests, benchmarks
│   ├── clash-verge-limiter/                          [Migrated]
│   │   └── Period checks, concurrent admission, existing tests
│   ├── Shared upstream components                   [Pending]
│   │   └── Logging, i18n, signals, media unlock as needed
│   ├── mihomo-client/                               [Migrated; Linux verified]
│   │   ├── Unix socket / explicit loopback HTTP      [Migrated]
│   │   ├── API methods, response models, errors      [Migrated]
│   │   ├── Realtime feeds, cancellation, reconnect   [Migrated]
│   │   └── Windows Named Pipe runtime validation    [Pending; code retained]
│   └── headless-core/                               [Partially migrated]
│       ├── Draft and limiter type re-exports        [Implemented]
│       ├── Subscription models and YAML schema     [Migrated]
│       ├── Runtime revisions, manifest, commit/recovery
│       │                                            [Implemented]
│       ├── Profile catalog, files, local import     [Implemented; upstream schema]
│       ├── Versioned settings store / explicit runtime fields [Implemented; Linux verified]
│       ├── Settings/runtime journal / interrupted-update recovery [Implemented; Linux verified]
│       ├── Full service settings and resource paths  [Pending]
│       ├── Profile selection / current mirror      [Implemented]
│       ├── Source controller removal / original YAML preservation [Implemented; Linux verified]
│       ├── Remote URL/YAML/header processing        [Migrated + adaptation]
│       ├── Remote raw-content import and metadata  [Implemented]
│       ├── Manual remote refresh + recovery journal [Implemented; direct/self_proxy/with_proxy]
│       ├── Metadata edit / noncurrent deletion     [Implemented; local/remote]
│       ├── Linked YAML merge storage / recovery    [Implemented; upstream schema]
│       ├── Linked rules/proxies/groups storage     [Implemented; upstream schema]
│       ├── Linked script storage / source / recovery [Implemented; upstream schema]
│       ├── Boa main(config, name), casing, console  [Migrated + worker adaptation]
│       ├── Raw read/edit / version guard / coordinated recovery [Implemented; Linux verified]
│       ├── Auxiliary cascade deletion / shared/reserved protection [Implemented; Linux verified]
│       ├── New-import auxiliary defaults / atomic catalog / recovery [Implemented; Linux verified]
│       ├── Enhancement: fields, merges, sequences   [Migrated]
│       ├── Reserved global Merge/Script defaults     [Implemented; upstream templates]
│       ├── Global/profile execution order and fallback [Migrated + staged adaptation]
│       ├── Transactional global editing / pointer recovery [Implemented]
│       ├── Explicit runtime settings authority       [Implemented; Linux verified]
│       ├── Typed DNS/TUN subset / shallow authority  [Implemented; Linux verified]
│       ├── Pure TUN/DNS derivation / IPv6 range repair [Migrated + staged adaptation; Linux validation]
│       ├── Provider DNS digest / profile preference / session confirmation [Migrated + adaptation; Linux verified]
│       ├── Deleted-profile DNS preference / confirmation cleanup [Implemented; recoverable]
│       ├── Hosts / native TUN integration            [Pending]
│       ├── Final LAN bind / group cleanup / field order [Migrated + staged adaptation; Linux verified]
│       ├── Full authoritative settings               [Pending]
│       ├── Runtime YAML + overlay generation        [Implemented; upstream merge reused]
│       ├── Profile enhancement generation          [Partially implemented; sequences/settings/TUN/DNS/global/profile/final stages]
│       ├── Per-profile node selection records       [Implemented; upstream schema]
│       ├── Geo/provider resources and proxy views   [Pending]
│       ├── Immutable revision / orphan file garbage collection [Pending]
│       ├── Timed update metadata / saved refresh source [Migrated + service scheduler]
│       └── Backup models / full upgrade resource settings [Pending]
├── service/                                         [Partially implemented]
│   ├── Persistent foreground entry point            [Implemented]
│   ├── Binary/data/config/import args, directory lock [Implemented]
│   ├── Listen/public-origin args and private token [Implemented; Linux verified]
│   ├── Built Web directory argument               [Implemented; --web-dir]
│   ├── Pinned resource directory / persistent managed core [Implemented; Linux]
│   ├── Startup settings snapshot / candidate authority [Implemented; Linux verified]
│   ├── Actor settings read/replace / coordinated apply and rollback [Implemented; Linux verified]
│   ├── DNS/TUN subset in generation / settings transactions [Implemented; Linux verified]
│   ├── Raw/enhanced candidate phases / single TUN derivation [Implemented; Linux validation]
│   ├── DNS conflict commands / scoped confirmation / coordinated auto-disable [Implemented; Linux verified]
│   ├── Final candidate LAN/group normalization after authority [Implemented; Linux verified]
│   ├── Geo/provider resources / full settings       [Pending]
│   ├── Core state watches and bounded log stream    [Implemented]
│   ├── Built-in proxy port readback / restart fallback and rollback [Implemented; Linux verified]
│   ├── Profile snapshots/watches and active UID     [Implemented]
│   ├── Local/file/remote imports with owned defaults / startup recovery [Implemented; Linux verified]
│   ├── Actor metadata edits / protected cascade / settings/file cleanup recovery [Implemented; Linux verified]
│   ├── Actor raw YAML read/edit / original and enhanced validation / coordinated commit [Implemented; Linux verified]
│   ├── Actor linked merge edits / detach / commit recovery [Implemented; Linux verified]
│   ├── Actor linked sequence edits / detach / commit recovery [Implemented; Linux verified]
│   ├── Actor linked script edits / detach / commit recovery [Implemented; Linux verified]
│   ├── Global/profile staged generation on select/refresh/edit [Implemented; Linux verified]
│   ├── Global read/set/reset, validation/apply/recovery [Implemented; Linux verified]
│   ├── Disposable script worker / limits / cancellation / reap [Implemented; Linux only]
│   ├── Remote HTTP(S) download + actor-backed import [Implemented; direct/self_proxy/with_proxy]
│   │   ├── Bounded download size/concurrency, timeout/cancel [Implemented]
│   │   ├── Manual refresh / stale-download guard / journal recovery [Implemented; Linux]
│   │   ├── Managed core proxy / live route / auth / lifecycle cancellation [Implemented; Linux verified]
│   │   ├── Service system proxy / environment / bypass / auth [Implemented; Linux verified]
│   │   ├── Native Windows/macOS proxy discovery runtime validation [Pending; library code retained]
│   │   ├── TLS platform/static roots / explicit certificate option [Migrated + adaptation; Linux verified]
│   │   ├── Scheduled refresh / retirement / bounded workers / drain [Migrated + adaptation; Linux verified]
│   │   └── SOCKS/PAC                               [Pending]
│   ├── Stable core release preparation                 [Partially implemented; Linux x86_64]
│   │   ├── Official latest/pinned metadata and platform asset [Implemented]
│   │   ├── Bounded compressed download / SHA-256 / private atomic cache [Implemented]
│   │   ├── Authenticated preparation/readback / cancellation [Implemented]
│   │   ├── Bounded gzip / ELF / version / configuration probes [Implemented; Linux x86_64]
│   │   ├── Actor snapshot / staged manifest / verified restart readback [Implemented]
│   │   ├── Actor activation / live version and port checks / rollback [Implemented; Linux x86_64]
│   │   ├── Durable switch journal / installation receipt / startup recovery [Implemented; Linux x86_64]
│   │   ├── Upstream force/no-op adapter / upgrade Web workflow [Pending]
│   │   └── Proxy routing / static-root fallback / Alpha / other targets [Pending]
│   ├── Node selection / unfix / persistence rollback [Implemented; Linux verified]
│   ├── Selection reconciliation and restoration    [Migrated + actor adaptation]
│   │   └── Startup keep-records, apply repair, bounded provider retries
│   ├── Full application context and domain events  [Pending]
│   ├── Sole Mihomo lifecycle manager                [Implemented; Linux verified]
│   │   ├── Start, readiness, stop, restart, recovery, reap
│   │   ├── Linux core/validator parent-death termination [Implemented; Linux verified]
│   │   ├── YAML / Mihomo -t validation and cancellation [Implemented]
│   │   ├── Serialized reload, restart fallback, rollback [Implemented]
│   │   ├── Runtime commit and interrupted-apply recovery [Implemented]
│   │   ├── Active profile + runtime commit/recovery [Implemented]
│   │   ├── Linked/global enhancement validation/apply/rollback [Implemented; Linux verified]
│   │   └── Full enhancement/resource transaction    [Pending]
│   ├── Axum management API / command adapters       [Implemented; MVP allowlist]
│   │   ├── State, logs, profiles, config, proxies queries [Implemented]
│   │   ├── Lifecycle, YAML import/edit/overlay, profile edit/delete/import/refresh, linked read/set/clear, global read/set/reset, settings read/replace, profile DNS read/set, raw profile read/edit and node selection [Implemented]
│   │   ├── Stable core query / preparation / staging / activation / installation readback [Implemented; Linux x86_64]
│   │   └── Broader rules/providers/connections/delay commands [Pending]
│   ├── HTTP bearer / WS first-frame auth, Host/Origin controls [Implemented; Linux verified]
│   ├── WebSocket events and realtime forwarding     [Implemented; Linux verified]
│   │   ├── State/profile snapshots, watches, log tail/reset [Implemented]
│   │   ├── Traffic, memory, connections/count, core logs [Implemented]
│   │   └── Per-session cancellation, bounded queues/retry/drain [Implemented]
│   ├── Web static assets and scoped SPA fallback    [Implemented; Linux verified]
│   ├── Unix SIGINT/SIGTERM and unified shutdown     [Implemented]
│   ├── User systemd unit template                  [Scaffold; static check only]
│   └── Windows SCM / other platform service integration [Pending]
├── web/                                             [Partially implemented]
│   ├── React build, login and responsive layout     [Implemented; MVP]
│   ├── HTTP commands, WebSocket events/feed adapters [Implemented; MVP allowlist]
│   │   └── Broader command and feed views           [Pending]
│   ├── Local profiles, config editor, core state    [Implemented; MVP]
│   │   ├── Remote URL import, usage display, saved auxiliary defaults [Implemented]
│   │   ├── Manual remote refresh / usage updates  [Implemented]
│   │   ├── Managed/system proxy import / saved mode editor / failed drafts [Implemented; Linux verified]
│   │   ├── Explicit subscription TLS option / saved edits / failed drafts [Implemented; Linux verified]
│   │   ├── Automatic interval/enabled editing / overdue startup workflow [Implemented; Linux verified]
│   │   ├── Metadata editor / confirmed cascade deletion [Implemented; Linux verified]
│   │   ├── Linked YAML merge editor / detach       [Implemented]
│   │   ├── Linked rules/proxies/groups editor / detach [Implemented]
│   │   ├── Linked script editor / detach / failure logs [Implemented; Linux]
│   │   ├── Global Merge/Script editor / reset / diagnostics [Implemented; Linux verified]
│   │   └── Raw subscription editor / conflicts / independent readback [Implemented; Linux verified]
│   ├── Proxy selection, unfix and bounded logs      [Implemented; MVP]
│   ├── Proxy connection information / actual ports / save verification [Implemented; Linux verified]
│   ├── Traffic, memory and connection-count overview [Implemented; MVP]
│   ├── Rules and full connection/provider views     [Pending]
│   ├── Runtime settings editor / inheritance / readback [Implemented; Linux verified]
│   ├── DNS/TUN editor / lossless nested inheritance / readback [Implemented; Linux verified]
│   ├── Provider DNS confirmation / cancellation / reconnect reconciliation [Implemented; Linux verified]
│   └── Full settings, core upgrade, backup UI       [Pending]
├── Release and deployment                           [Partially implemented]
│   ├── Linux x86_64 bundle: Rust + independent Mihomo + Web [Implemented]
│   ├── Explicit target/version/SHA-256 resource manifest [Implemented]
│   ├── Writable persistent core initialization      [Implemented; Linux verified]
│   ├── One foreground exec launcher                 [Implemented; Linux verified]
│   ├── Preserve data and existing upgraded core      [Implemented; Linux verified]
│   ├── Managed core installation receipt / interrupted-switch recovery [Implemented; Linux x86_64]
│   ├── User systemd template deployment              [Scaffold; runtime pending]
│   ├── Other platforms, containers, Alpha/Geo resources [Pending]
│   └── External publication/license resolution      [Pending]
└── Documentation and provenance                     [Implemented; maintained]
    ├── headless.md
    ├── docs/UPSTREAM.md
    ├── docs/ARCHITECTURE.md
    ├── docs/RUNNING.md
    └── docs/DEPLOYMENT.md
```

The runtime boundary is a browser communicating over HTTP/WebSocket with one
Rust management service. That service owns configuration workflows and is the
sole lifecycle manager for a separate Mihomo child process. The browser does
not access the internal core controller directly. Production does not require
a Node or Vite process.

The binary now runs continuously, publishes core state as JSON on stdout, and
manages one Mihomo child with serialized lifecycle operations and graceful Unix
shutdown. It can start with `examples/minimal.yaml`; see
[running instructions](RUNNING.md). The authenticated Axum listener is available
before core startup and while the core is stopped or failed. A minimal React UI
is available when `--web-dir ./web/dist` supplies built assets, including failed-core
repair, local profiles, runtime editing, nodes, logs and realtime metrics.
Direct, managed and system-proxy remote import and manual refresh preserve downloaded YAML, upstream metadata
and profile identity. Active refresh uses validated application and recoverable
commit; linked sequence, YAML merge and script editing feed selection and refresh generation.
Automatic scheduling, TLS root fallback and explicit per-subscription bypass are connected. The remaining enhancement workflows are incomplete. Local profile imports,
selection and restoration now feed the runtime validation/application flow. Runtime YAML imports,
upstream merge overlays, validation, application, persistence, and interrupted
application recovery work through the manager; `--import-config` exposes the
initial runtime file import in the foreground CLI. `--import-profile` saves and
selects local YAML, while `--select-profile` chooses a persisted UID.
**The initial Linux local-profile MVP is now verified through the bundle launcher.**
This does not establish full feature parity or other deployment/platform support.

The runtime configuration flow uses the extracted sequence/merge helpers and adapted
Boa script processor with global defaults and saved per-profile links; authoritative
explicit settings, DNS/TUN field authority and pure TUN/DNS derivation are connected;
provider DNS protection and confirmation are connected. Final LAN binding,
proxy-group cleanup and upstream field order run after authority and before
validation. Native TUN integration, hosts settings and the full resource pipeline
remain pending. Subscription schema extraction includes
`IProfiles`, `PrfItem`, `PrfSelected`, `PrfExtra`, and `PrfOption`. The new profile
store persists the upstream `profiles.yaml` and `profiles/<file>` layouts, retains
metadata and recorded node selections, and adapts local import/current lookup
without desktop globals. Direct fetching/import, manual refresh, metadata editing
and noncurrent deletion are connected. Linked merge, sequence and script items can be read, saved,
replaced and detached; selection and active refresh apply their saved content.
Global Merge/Script defaults initialize after journal recovery, preserving existing
rows and source files. Raw read/edit uses versioned immutable source publication
and coordinated active-runtime recovery. Noncurrent cascade deletion protects
shared/reserved auxiliaries and files and cleans deleted DNS preferences. New service
imports initialize owned upstream defaults through a recoverable single catalog
commit. Scheduled updates and additional remote download modes remain pending. Generation applies rules, proxies
and groups sequences, global merge/script, then profile merge/script. Unlinked
profile stages reuse reserved defaults, including the upstream double application
of globals. Authenticated global read/set/reset commands now coordinate catalog
file pointers with runtime commit/recovery; the subscription page exposes global
editors, confirmed template reset and failure diagnostics.
Scripts run only through a cancellable, resource-limited Linux worker process;
other platforms fail explicitly until equivalent limits are implemented.
Sequence operations preserve upstream node-reference cleanup and insertion
into the first selector group. DNS shallow merge and hosts replacement semantics
are retained by the pure merge helper. The typed DNS/TUN settings and browser
editor now integrate the supported subset; the complete pipeline remains pending.

The standalone Mihomo client retains the complete upstream API implementation
and uses ordinary Rust callback messages and Tokio tasks. Verification covers
Linux HTTP and Unix socket queries, node selection, configuration patch/reload,
realtime feeds, consumer cancellation, and explicit resubscription against a
real core. Remote provider fetches, delay checks, Geo downloads, and upgrade
operations are retained but still need live validation. The service lifecycle
manager is now implemented separately; HTTP proxy queries are connected, while
browser WebSocket forwarding is connected and broader client command adapters
remain pending. The pinned plugin
does not declare a license; its publication status is recorded separately in
`UPSTREAM.md`.

The supervisor holds the data-directory ownership lock, drains output from
spawn, probes the private controller, records failures without stopping the
foreground service, and bounds automatic recovery. Reload operations use the
same serialized command flow as start/stop/restart. Candidate YAML is stored as
an immutable revision, checked by `mihomo -t`, applied via reload or restart,
then committed in an atomic manifest. Failed application restores the previous
manifest and attempts to restart the previous core configuration; recovery errors
are reported. A pending revision after service interruption is discarded in favor
of the committed one. The runtime journal commits the active profile UID with the
runtime revision. `profiles.yaml.current` is a compatibility mirror repaired from
the journal on startup; a failed switch restores the prior UID and configuration.
Node selections now restore on starts, restarts, and accepted configuration/profile
changes. Restoration is serialized by the same actor, retries incomplete groups,
and is superseded by manual node operations and lifecycle changes. Startup keeps
unavailable records; config/profile application can reconcile stale records after
confirmation. Linked merge/sequence/script changes coordinate catalog and runtime commit/recovery.
Complete enhancement/resource transactions,
provider/Geo resource rollback, and structured validation outcomes remain pending.
Windows console/pipe/storage behavior remains unverified, and SCM and
abnormal-exit platform cleanup require deployment integration.

## Previous increment: authenticated HTTP management

Delivery step 5 now has a functioning HTTP command boundary. Axum 0.8.9 exposes
authenticated status/log/profile/config/proxy reads and an explicit command
allowlist. Lifecycle, profile upload/selection, full YAML application, overlays,
node selection and unfix reuse the manager; they do not bypass validation,
serialization or persistence. Browser uploads supply content rather than server
paths. Config reads and content imports also run through the actor.

`--listen` defaults to `127.0.0.1:9090`; `--public-origin` sets the exact public
origin/Host, and wildcard listening requires it. An atomic, private
`management-token` file holds a persistent random bearer credential. HTTP checks
duplicate credential headers, Host and Origin before parsing JSON; credentials
are not accepted from queries or cookies. Requests have a 9 MiB JSON-body limit,
with separate 8 MiB domain YAML limits. Responses/errors are JSON and non-cacheable;
unknown routes return 404 without an SPA fallback. Core queries time out after
ten seconds. Shutdown closes command admission and the listener before cancelling
core operations and reaping the child, then bounds HTTP draining to ten seconds.

Verification: workspace check, formatting, Clippy with warnings denied, and all
81 regular workspace tests pass. Five new regular tests cover credential storage,
origin/listener/file restrictions, authentication before parsing, duplicate
headers, unknown commands/fields, request limits, JSON errors, shutdown admission,
and actor-backed content import/config access. A real TCP HTTP test against
Mihomo v1.19.31 exercises missing-bootstrap failure, API repair, import/select,
start, node persistence, rejected invalid rules, valid reload, stop/restart,
service restart/restoration, logs and SIGTERM child reaping. Existing lifecycle
and node integration tests also pass after the entry-point change (13 explicitly
run service integration tests total: nine real-core tests and four controlled
socket tests). Socket tests remain opt-in in the default test run.
This session uses `CARGO_HOME=/tmp/mihomo-server-cargo` for a writable dependency
cache because the environment mounts the default Cargo cache read-only; ordinary
builds still use the checked-in lockfile and normal registry dependencies.

The HTTP slice established the command boundary used by subsequent increments.
At that stage, React UI/static serving, resources and deployment were still pending;
the following increments connect the initial MVP path.

## Previous increment: WebSocket events and realtime forwarding

Delivery step 5 now includes `/api/events` and `/api/streams/{feed}` for `traffic`,
`memory`, `connections`, `connections_count` and `logs`. Browser upgrades check
the same Host/Origin policy, then require an `authenticate` token frame within
five seconds. No snapshots or internal controller subscriptions are created
before token validation. Credentials never appear in event bodies or query URLs.
HTTP bearer commands retain their previous contract.

Events provide a fresh status/profile/log snapshot on every authenticated
connection, then watch updates and core output. Lagged log consumers receive a
fresh tail and subscription. Realtime sessions use the extracted client's checked
callbacks, with independent subscription guards and targeted synchronous
cancellation. Stopped/failed cores remain observable; streams wait for Running,
drop old subscriptions on lifecycle/generation changes, and reconnect after core
stream closure or failure with a one-second retry delay. Browser transport
reconnection still requires a new authenticated socket, which receives fresh state.

The service admits at most 32 upgraded sessions, limits inbound messages to 4 KiB,
queues at most eight samples of up to 1 MiB per realtime session, bounds outgoing
events to 16 MiB and socket writes to two seconds, and sends heartbeat pings.
Overflow closes that session instead of silently dropping samples or accumulating
unbounded queues. Unified shutdown closes admission, cancels pending authentication
and core connection attempts, cancels owned subscriptions and waits for upgraded
tasks as well as HTTP connections. The existing ten-second management drain
deadline now covers both; Axum's HTTP shutdown alone does not await upgraded tasks.

Verification includes the workspace check, all 82 regular tests and 18 explicitly
run workspace socket integration tests, formatting and Clippy with warnings denied.
Three new socket integration tests cover token/frame
rejection, Host/Origin checks, no data before authentication, authentication timeout,
session admission limits, pending-session shutdown, failed/stopped-core snapshots,
profile events and reconnect snapshots. Real Mihomo tests exercise all five feeds,
independent consumers, targeted cancellation/retry, hot reload, core stop/start,
generation replacement and subscription cleanup. The real CLI HTTP workflow now
keeps authenticated and pending WebSockets open across SIGTERM and verifies their
closure with successful process shutdown and core reaping.

That increment supplied the transport used by the browser UI below. The later
bundle increment supplies explicit initial resources and deployment verification.

## Previous increment: minimal React UI and Rust-served assets

Delivery step 5 now has a working browser path. React 19.3, TypeScript 6.0.3 and
Vite 8.3 build independent assets; the foreground Rust service serves them when
`--web-dir` is supplied. Node/Vite are build tools, not production processes.
Omitting the argument retains API-only operation. An explicitly missing or unsafe
asset directory fails management initialization before the core starts.

The responsive UI provides token login, core state/start/stop/restart, local YAML
file/content import and profile selection, runtime YAML editing, proxy-group node
selection/unfix, a bounded core-output viewer, and traffic/memory/connection-count
metrics. HTTP commands and first-frame-authenticated WebSocket adapters use only
the management listener. Tokens stay in React memory; refresh requires login.
Event reconnection receives current snapshots, feed views clear stale samples,
and view disposal/logout closes owned sockets and cancels pending HTTP requests.
Core startup/validation failures remain visible and repairable in the browser.

Assets are public so login can load, while API reads and commands retain bearer
authentication. Both enforce the configured Host/Origin policy. Static serving
canonicalizes the root and requested files, rejects traversal/external symlinks,
and applies CSP, no-store and nosniff. Only the five known browser navigation paths
can fall back to index.html; unknown assets and API routes remain JSON errors.
HEAD, MIME handling, API-only operation and invalid asset roots are verified.

Browser configuration editing revealed a persistence distinction: standalone
`apply_config` intentionally detaches the active profile. The new `edit_config`
replaces the complete runtime mapping while preserving the active UID, so node
records still restore after edits and service restart. It uses the same validation,
serialized application, journal, profile mirror and rollback as existing commands.
The browser editor uses this command; import/apply and overlay semantics remain
unchanged. The full original profile source is not overwritten by runtime edits.
Real-core regression also exposed disabled TUN returning `dns-hijack: null`;
the extracted model now treats null as an empty list while rejecting invalid
list elements and preserving its existing serialization.

Verification: `cargo check --workspace`, formatting and Clippy with warnings denied,
all **86 regular workspace tests**, all **18 opt-in socket integration tests**,
and the production frontend build pass. One real Chromium browser workflow uses
the Rust-served build and Mihomo v1.19.31, without a desktop: invalid/valid login,
failed-bootstrap repair, local file import/select, start, node choice, invalid/valid
YAML editing, stop/restart, logs and actual realtime samples, full service restart
with config/profile/node restoration, WebSocket reconnect, deep-link reload,
mobile layout, logout and successful SIGTERM cleanup. Three new regular static
asset tests and one null-TUN model regression supplement the existing actor tests.

That increment completed Delivery step 5. The bundle/launcher increment below
supplies the initial deployment path; remote subscriptions, full enhancement,
advanced UI and platform service/release features remain required work.

## Previous increment: pinned Linux bundle and persistent managed core

Delivery step 6 now provides the initial verified deployment path. The standard
Python packager accepts an existing independent core, an explicit version/target
and an expected SHA-256. It rejects mismatched checksums/versions/ELF architecture,
unsafe Web resources and existing output directories. Optional `--build` runs
locked npm/Cargo builds and creates a fresh Linux x86_64 release bundle containing
the Rust service, core, built Web assets, bootstrap YAML, manifest, checksums,
launcher and provenance. It does not download floating core releases or modify
runtime data. The real `--build` release compilation and package preparation pass.

`--resource-dir` resolves bundle resources and conflicts with external `--mihomo`.
The target must match the service build target. Web/bootstrap paths can be
explicitly overridden. The lifecycle manager holds its data lock before verifying
and initializing `<data-dir>/core/verge-mihomo`, or an explicit `--core-dir`.
First-use initialization hashes the bytes being copied, syncs a private staging
file and atomically publishes without replacing an existing executable. Existing
managed cores are authoritative, preserving independently upgraded files even
when bundle contents change. Unsafe links, permissions and malformed manifests
are rejected. Actual core upgrade/download/rollback operations remain pending.

The launcher requires an absolute persistent data path, sets a private umask and
execs the Rust service. Launching from an unrelated working directory works;
Node/Python/Vite are not runtime processes. Service updates use fresh bundle paths
and retain data/core directories. An optional one-service user systemd template
uses SIGTERM, mixed control-group cleanup and a 30-second deadline. Its rendered
unit passes `systemd-analyze verify`, and shell syntax passes `sh -n`; no systemd
unit was installed or run, so that deployment remains explicitly unverified.

Verification passes workspace check, Clippy with warnings denied, formatting,
**91 regular Rust tests**, **19 opt-in socket/deployment integration tests** and
**6 Python packaging boundary tests**. The new real bundle integration covers
failed-bootstrap page availability, import/select/start, persisted node choice,
invalid/valid runtime edits, stop/restart, service restart from a relocated bundle,
retained credentials/config/profile/node records and a changed managed-core file,
and successful SIGTERM shutdown with the owned core absent after service reaping.
The real Chromium workflow also passes against the **release bundle launcher**,
covering production assets, repairs, restoration/reconnect, realtime samples,
mobile layout and logout. All test services and cores are cleaned up.

**The initial Linux local-profile MVP meets the boundary below.** Full project
completion remains false: remote download/refresh, linked enhancements, advanced
settings/pages, timed work, core upgrades, backup and platform release integrations
are pending. The remote subscription increment below supplies direct content
download/import; refresh/scheduling and complete enhancement remain later work.
See [deployment instructions](DEPLOYMENT.md) for the verified launcher path.

## Previous increment: direct remote subscription download and import

Delivery step 7 now connects URL download/import through `import_remote_profile`
and a browser form. The service uses the existing pinned reqwest client for direct
HTTP(S), disables environment proxies, retains the upstream user-agent default,
ten-redirect policy and actual 20-second default timeout, and checks response status.
Overrides support user agent, a bounded 1..120-second timeout, update interval and
auto-update metadata. Unsupported proxy/TLS-bypass/enhancement options are rejected
rather than ignored. Platform TLS is used; desktop TLS fallback and live external
provider/HTTPS verification remain pending.

Pure upstream URL repair, header parsing and subscription checks now live in
headless-core without HTTP dependencies: dirty path queries, filename/filename*,
standard or storage-prefixed subscription-userinfo, homepage normalization,
hours-to-minutes update intervals, BOM removal and required proxies/provider keys.
Safe adaptations bound URLs/names/body sizes, reject interval overflow and require
UTF-8. Remote imports use `R` UIDs and upstream metadata/file formats, persist raw
content privately, and publish catalog snapshots without activating or replacing
existing profiles. Selecting the imported UID separately validates/applies through
the existing manager; saved content and node records restore without refetching.
Automatic auxiliary enhancements are still omitted, as for local imports.

Network futures run outside the lifecycle actor, with four permits shared across
manager clones and held through import completion. Only completed, validated
content enters the serialized actor/store transaction. Slow downloads therefore
do not block core operations or supervision. Shutdown cancels active downloads
and waiting admission; abandoned queued imports are skipped. Content-Length and
streamed chunks are bounded to 8 MiB before parsing. Management credentials are
never forwarded to providers, and transport errors remove subscription URLs.
Downloads do not bypass TLS certificate validation or fall back to a core proxy.

Verification: workspace check, formatting and Clippy with warnings denied,
**97 regular Rust tests** and **23 opt-in socket/deployment integration tests**
pass. Three pure processing tests, two persistence tests and one regular option
boundary test were added. Four new socket tests cover HTTP authentication before
fetch, redirects/headers, metadata, invalid/status/size/timeout failures, unchanged
catalog/runtime, bounded admission, responsive actor operations and shutdown.
Real Mihomo activation/node selection/service restoration uses the downloaded
profile and proves startup makes no additional provider request. The Chromium
workflow adds actual provider download, failed import preservation, remote usage
rendering and catalog persistence across restart while retaining the original
lifecycle/config/node/mobile checks. Frontend and release builds pass.

The initial Linux MVP remains runnable. **The full project is not complete.**
At that increment, manual remote refresh with identity/node preservation and
active-profile validation/recovery was the next subtask; it is implemented below.
Proxy/TLS fallback, full enhancement, advanced settings/UI, upgrades, backups
and other deployment/platform integrations remain required work.

## Previous increment: transactional manual remote profile refresh

Delivery step 7 now provides authenticated `refresh_profile { uid }` and a remote
profile button. Manual refresh reuses persisted download options, including when
allow_auto_update is false. It preserves UID, user title, description, original
URL and recorded nodes, updates usage/home/timestamp metadata, and reuses the
upstream PrfOption::merge helper. Active refresh regenerates from the downloaded
raw profile, replacing runtime-only edits, as reselecting a profile does. Valid
node records survive; the existing upstream reconciliation can remove unavailable
records after successful application. Noncurrent refresh updates only cached
content/catalog and does not change the running revision/PID or activate the UID.
A stopped current profile is validated/committed without starting the core.

Downloads share the existing four permits with imports, remain outside the
lifecycle actor and cancel on service shutdown. The actor checks the source
file/URL/options before committing; competing responses cannot overwrite an
already refreshed profile. Current node records are read at commit time, so manual
node choices made during download survive. Selection changes during download are
handled according to the active UID at commit, not the initial network snapshot.

The profile store writes private immutable `profiles/refresh-*.yaml` candidates
and a synced `profile-refresh.yaml` journal. It changes only the affected item's
catalog pointer, retaining old content. Active refresh stages and checks `mihomo -t`,
then reloads/restarts, publishes profile metadata and commits the runtime journal.
Validation failures preserve old content/catalog/runtime; application or catalog
failures restore the prior journal, raw-content pointer and working core when
possible, with recovery failures reported. On startup, an interrupted active
refresh follows the committed runtime revision; an inactive refresh follows the
catalog rename. Pending runtime revisions are never promoted. Recovery runs before
further actor commands, and journal cleanup errors are reported without silently
undoing a successful commit. Retained raw/runtime revisions still need garbage
collection; this is a scoped YAML/profile transaction, not full resource rollback.

The storage ordering intentionally strengthens the desktop update workflow, which
saves a subscription before confirming core application. UID, metadata and option
semantics follow upstream; immutable cache pointers and coordinated recovery are
service adaptations documented in UPSTREAM.md.

Verification: `cargo check --workspace`, formatting, Clippy with warnings denied,
**102 regular Rust tests**, and **26 explicitly run opt-in socket/deployment tests**
pass. Five added pure/store tests cover option merge, metadata/node preservation,
interrupted active/inactive commits, catalog write failure and unsafe journal/content.
Three added socket tests cover real-core active/noncurrent/stopped refresh,
validation failure, post-reload catalog failure, reload-plus-restart failure recovery,
node changes during download, stale competing downloads and shutdown cancellation.
The HTTP test now verifies authentication before refresh, strict fields and unchanged
UID/count. Both Chromium workflows pass against the newly built Linux release
bundle, including refresh usage, active application, invalid update preservation,
UID/node/config restoration after service restart and no startup refetch.
Frontend/release builds pass; test services and owned cores are cleaned up.

The initial Linux MVP remains runnable; **the full project is not complete**.
At that increment, metadata editing/deletion with current-profile protection and
consistent file/catalog updates was the next subtask; it is implemented below.
Full linked enhancement workflows precede scheduled updates in the delivery order.
Proxy/TLS fallback, broader settings/UI, upgrades, backups and additional platform
release/service integrations remain pending. Auto-update flags remain metadata;
no scheduler runs in this increment.

## Previous increment: profile metadata editing and protected deletion

Delivery step 7 adds authenticated edit_profile { uid, patch } and delete_profile
{ uid }, a metadata editor and inline deletion confirmation. Local/remote titles
and descriptions can be edited; remote URL, user agent, 1..120-second timeout,
update interval and auto-update metadata can be patched. Nested fields are strictly
allowlisted. UID/type/file/usage/timestamp/node records and enhancement references
remain owned by the store. Validated patches merge supported option fields and
atomically save the catalog, preserving raw content, active UID, node records and
running revision/PID. They do not download or activate content. Old usage/home
metadata remains the last successful fetch until manual refresh updates it.

Deletion is serialized by the same actor, protects the runtime active UID even
while stopped, and rejects the current compatibility mirror. It requires an explicit
switch to another profile first; it does not auto-select/restart a replacement.
Linked profiles and enhancement references are rejected until ownership/generation
are migrated. A catalog with shared file pointers retains content still referenced
by another item. Deleting an ordinary local/remote item removes its node records
and current raw file without changing the working runtime.

A private profile-delete.yaml journal stages deletion before catalog publication.
Catalog rename commits removal, then the file is cleaned up. A present UID on
startup aborts an uncommitted plan; an absent UID completes cleanup. Catalog-write
failures preserve content; cleanup failures leave a journal, report the committed
catalog outcome and retry before subsequent commands. Missing files are tolerated,
unsafe names/symlinks/directories are rejected. Older unreferenced raw/runtime
revisions remain retained pending garbage collection. The actor recovers refresh
and deletion journals before further operations. An in-flight refresh cannot
recreate a deleted UID; title/description edits are read at refresh commit and
URL/option edits reject its stale source snapshot.

Verification: workspace check, formatting, Clippy with warnings denied,
**109 regular Rust tests**, **27 explicitly run opt-in socket/deployment tests**,
production frontend/release builds and **three Chromium workflows** pass. Six new
store tests cover metadata preservation,
input/write failures, current protection, interrupted deletion/cleanup retry,
shared files, enhancement relationships and unsafe paths. One new HTTP test
checks authentication, strict patches, owned fields and protected deletion.
One new real-core/provider test checks no runtime/network changes on edits,
current/stopped protection, edits/deletion during refresh and restart persistence.
The browser workflow adds local/remote edits/removal, option persistence, cancellation
of deletion, protected current state, actual file cleanup, restart and mobile layout.

The Linux MVP remains runnable; **the full project is not complete**. At that
increment, linked YAML merge persistence and runtime generation was the next
Delivery step 7 subtask; it is implemented below. Sequence/script/DNS/TUN
integration, linked deletion, scheduling, proxy/TLS modes, upgrades, backups,
advanced pages and other platforms remain pending. Metadata editing does not edit
raw subscription content or implement automatic scheduling.

## Previous increment: linked YAML merge persistence and runtime generation

Delivery step 7 adds authenticated `profile_merge { uid }`,
`set_profile_merge { uid, yaml }` and `clear_profile_merge { uid }`, plus a browser
YAML editor and detach action. Local/remote base profiles keep their UID, raw file,
metadata and node records. Each save creates an immutable lowercase `m` UID/file
with upstream type `merge` and timestamp, and updates `PrfOption.merge`. The API
catalog retains auxiliary rows; the browser lists only local/remote base profiles.
Unknown fields and null/missing YAML are rejected; clearing is an explicit command.

Selection and active refresh generate from raw YAML plus the linked merge through
the extracted upstream helper. Overlay top-level keys normalize to lowercase;
ordinary mappings merge recursively, arrays replace, DNS fields merge shallowly
and hosts replace. The service enforces its private controller boundary after
generation. It does not yet implement desktop authoritative settings, the global
Merge/Script chain, linked sequence/script execution or DNS/TUN settings management.
Imports still do not automatically create auxiliary/default items.

Current-profile edits/detach stage a runtime revision and validate `mihomo -t`
before saving/publishing the auxiliary link. They reuse serialized reload/restart,
rollback and node restoration. A stopped current profile stays stopped. Inactive
edits check mapping/size/controller rules and publish storage only; core validation
occurs on selection. Remote refresh retains the link and resolves its saved content
at actor commit; changing options during a download rejects that stale response.
Runtime-only edits are replaced when regenerating from raw plus merge.

A private, bounded `profile-merge.yaml` journal contains previous/candidate catalogs
and the expected runtime revision. Candidate content is written/synced first; catalog
publication links it, then current-profile runtime commit accepts the transaction.
Interrupted active edits follow the committed runtime revision; inactive edits follow
the catalog link. Recovery runs on startup and before further actor commands and
never promotes pending runtime revisions. Failed validation/application/catalog save
preserves or restores the old link, raw content, active UID and working core, with
recovery errors reported. Shared old merge rows and the reserved global `Merge` row
are preserved; unreferenced ordinary old rows are retired, while immutable content
is retained pending garbage collection. Base deletion still requires detaching links
and switching away from the current UID; there is no auxiliary cascade deletion.

Verification: `cargo check --workspace`, formatting, Clippy with warnings denied,
**115 regular Rust tests**, **29 explicitly run opt-in socket/deployment tests**,
production frontend/release builds and **four Chromium workflows** pass. Five new
store tests cover upstream merge semantics, raw/node preservation, strict inputs,
shared links, active/inactive interruption outcomes, catalog failures and unsafe
content/journals. One new HTTP test covers authentication, strict read/set/clear
shapes, private-controller restrictions and active/stopped application. Two new
real/controlled-core tests cover selection/refresh, invalid rules, post-reload catalog
failure, reload-plus-restart failure recovery, node retention and restart persistence.
The browser workflow covers raw preservation, auxiliary-row filtering, saving and
replacing merges, invalid-update preservation, restart restoration and detach.
The existing client fixture now waits for its proxy/rule data as well as controller
availability, fixing a startup race observed during full integration verification.
The fresh Linux release bundle at `target/mihomo-server-linux-x86_64-merge` is
launcher-tested and every packaged SHA-256 checksum passes; owned test cores are reaped.

The usable Linux MVP remains verified; **the full project is not complete**.
At that increment, linked rules/proxies/groups sequence persistence and generation
was the next Delivery step 7 subtask; it is implemented below.
Scripts/global chain, raw editing, full DNS/TUN/service settings, cascade deletion,
scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional
platform deployments remain pending.

## Previous increment: linked rules/proxies/groups sequence enhancements

Delivery step 7 adds authenticated `profile_sequence { uid, kind }`,
`set_profile_sequence { uid, kind, yaml }` and `clear_profile_sequence { uid, kind }`,
with strict `kind` values rules/proxies/groups and a browser type selector/YAML editor.
Each save creates an immutable upstream auxiliary item: lowercase r/p/g UID, type
rules/proxies/groups, matching YAML filename, timestamp and base PrfOption link.
Base UID/raw content/metadata/node records are preserved. Existing imports still do
not create empty auxiliary/default items, and auxiliary rows remain excluded from
browser base-profile cards while API catalog queries retain them.

Sequence YAML requires explicit prepend/append/delete lists. Unknown fields,
null/missing lists, empty rules/deleted names, non-string rules and proxy/group
entries lacking mapping name/type are rejected. This is a strict service input
adapter around the unchanged extracted SeqMap/use_seq; Mihomo-specific semantics
are checked by the real core when applying. Generation follows upstream
process_seq_items: rules, then proxies, then groups, followed by the saved merge.
Proxy deletion cleans group references; new proxy names enter the first existing
selector before group edits. A later merge can replace sequence-produced arrays.
The full global/script, authoritative service settings and final desktop cleanup
pipeline remain pending; private-controller ownership is still enforced.

Merge and sequences share the actor-backed enhancement transaction and bounded
catalog journal. The historical profile-merge.yaml filename/schema version is
retained for interrupted-upgrade compatibility; a new strict kind field identifies
the affected link and defaults to merge for older records. Recovery validates its
matching auxiliary type/content. Current edits/clear validate -t before link
publication, then reload/restart/rollback with node reconciliation. Stopped current
profiles remain stopped; inactive edits receive core validation on selection.
Startup restores committed runtime/catalog without provider refetch. Remote refresh
regenerates all saved sequences plus merge, and link changes invalidate in-flight
source snapshots. Shared/reserved auxiliary rows survive detach; unreferenced ordinary
rows are retired, with immutable files retained pending garbage collection.
Base deletion still requires detaching all links and switching away from the active UID.

Verification: workspace check, formatting, Clippy with warnings denied,
**120 regular Rust tests**, **31 explicitly run opt-in socket/deployment tests**,
production frontend/release builds and **five Chromium workflows** pass. Four new
store tests cover upstream sequence order, merge precedence, raw/node preservation,
strict inputs, shared links, active/inactive interruption outcomes, journal kind
validation, legacy merge recovery, publication failures and unsafe content. One
HTTP test checks authentication and strict read/set/clear inputs. Two real/controlled
core tests verify selection/refresh, invalid candidates, post-reload catalog failure,
reload-plus-restart failure, stale downloads, stopped edits and restart/node recovery.
The fifth browser workflow checks all three types, raw preservation, invalid-update
preservation, service restart/node restoration, mobile layout and detach. The fresh
Linux bundle at target/mihomo-server-linux-x86_64-sequences is launcher-tested and
its packaged SHA-256 checksums pass; owned test processes are cleaned up.

The Linux MVP remains runnable; **the full project is not complete**.
At that increment, bounded linked script execution was the next Delivery step 7
subtask; it is implemented below.
Global enhancement defaults/order, raw editing, full DNS/TUN/service settings,
final group cleanup, cascade deletion, scheduling, proxy/TLS modes, upgrades/backups/
WebDAV, advanced pages and additional platform deployments remain pending.

## Previous increment: bounded linked script execution and transactional application

Delivery step 7 adds authenticated profile_script { uid }, set_profile_script
{ uid, source } and clear_profile_script { uid }, plus a JavaScript editor and
detach action. Scripts use upstream s UID/type script/<UID>.js/timestamp and
PrfOption.script, with immutable source and preserved base UID/raw/metadata/nodes.
Read returns source/UID or nulls; source is required, nonempty and at most 1 MiB.
Unknown fields, null/missing source, unsafe/nested/wrong-type links are rejected.
The shared enhancement journal now supports kind script; old merge journals retain
their default-kind compatibility. Shared/reserved Script rows survive detach;
retired ordinary source files remain pending garbage collection.

Boa is pinned to upstream 0.22.0 with its default features. Adapted evaluation
passes a lowercased configuration and safely bound profile name to synchronous
main(config, name), then lowercases the returned mapping. Console methods retain
upstream JSON formatting, with at most 1000 entries/1 MiB, and return diagnostics
through the existing authenticated bounded log stream. Scripts run after sequences
and per-profile merge on selection, active refresh and enhancement edits. Returned
arrays/null/Promises, syntax/execution/serialization failures and nonempty private
controller fields are rejected. Global ordering/defaults, authoritative service
settings, final group cleanup and DNS/TUN management remain pending.

Each evaluation runs in a disposable instance of the same Rust binary, before
creating any service/Tokio runtime. Linux worker limits are 512 MiB virtual address
space, 5 CPU seconds and no core dumps; parent wall time is at most 5 seconds,
with cancellation on shutdown, bounded stdin/stdout/stderr and explicit kill/reap.
Boa also retains the upstream 10-million loop limit. IPC is bounded to 20 MiB,
config JSON to 10 MiB and resulting YAML to 8 MiB. No filesystem/network/Node host
APIs are registered, no promise jobs are run, and worker environment is cleared.
This is resource/process isolation, not a general OS security sandbox. Other
platforms reject scripts until equivalent process limits are implemented.

Inactive saves execute the proposed script but defer Mihomo validation to selection.
Current edits/clear execute generation and -t before publishing source/link, reuse
reload/restart/rollback and keep stopped profiles stopped. Failure preserves the
working catalog/runtime/core and reports errors; captured console logs survive
evaluation failures. Unlike upstream's unchanged-config fallback, the service
rejects failed scripted transactions explicitly, preventing silent acceptance of
an enhancement that did not execute. Worker crash/timeout/cancellation may have
no console transcript. Startup uses committed runtime snapshots without executing
scripts or refetching; reselect/refresh regenerates. Changing enhancement options
supersedes stale downloads through the existing source guard.

Verification: workspace check, formatting, Clippy with warnings denied,
125 regular Rust tests, 35 explicitly run opt-in process/socket/deployment tests,
production frontend/release builds and six Chromium workflows pass. Four new
core/store tests cover main/name/casing, absent host APIs, failure diagnostics,
controller ownership, source/schema/raw/node/shared-link preservation and active/
inactive interruption recovery. One HTTP test checks authentication, strict fields,
execution rejection, logs and detach. Two real/controlled-core tests verify merge/
script order, refresh, validation/catalog/restart failure rollback, stopped edits,
restart/node restoration and shutdown during execution. Two worker tests cover
wall timeout, memory exhaustion, console/result limits and cancellation/reaping.
The sixth browser workflow verifies source retention, logs, rejected syntax/
execution/core validation, replace/restart, mobile layout and detach against the
fresh target/mihomo-server-linux-x86_64-scripts bundle; all SHA-256 checksums pass
and owned test processes are cleaned up.

The Linux MVP remains runnable; the full project is not complete.
Next Delivery step 7 subtask: global Merge/Script defaults and execution ordering.
Raw editing, authoritative service settings, full DNS/TUN, final group cleanup,
cascade deletion, scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced
pages and additional platform deployment/process isolation remain pending.

## Previous increment: global defaults and staged execution order

Delivery step 7 persists the upstream reserved Merge/Script defaults and connects
staged generation to selection, active refresh and linked enhancement edits.
Order is rules → proxies → groups sequences → global Merge → global Script →
profile merge → profile script. Missing profile links reuse reserved defaults,
so an unlinked profile applies global merge/script twice, as upstream does.
Reserved sequence rows are consumed if present; missing ones are empty defaults.
Editor read/clear APIs continue reporting explicit links, so clearing an enhancement
returns to its effective fallback rather than disabling the global stage.

The global Merge template sets profile.store-selected true; the Script template
is identity main(config, profileName). Exact identity source skips worker creation.
Custom global/profile scripts use separate bounded, cancellable workers and retain
console diagnostics. Generation now carries explicit stage data; the pure mapping
API refuses custom scripts instead of silently omitting them. Existing private
controller guards, Mihomo validation, runtime commit, rollback and node restoration
remain in force. Startup restores committed runtime without running scripts;
broken saved global sources reject regeneration while the working snapshot remains
available.

Initialization happens after journal recovery, writes unique private immutable
files and publishes missing rows together in the catalog. Existing rows, raw
profile content, metadata and node records are preserved. It is idempotent and
rejects pending journals; ordinary publication failure removes unpublished files.
Interrupted creation may leave unused files for future garbage collection.
Existing catalogs with items null are preserved, matching upstream. Invalid types,
nested references, unsafe files and malformed enhancement content fail generation.

Workspace check, formatting and Clippy with warnings denied pass; all 129 regular
Rust tests and 36 explicitly enabled integration tests pass. Four new core/store
tests cover initialization preservation, publication failure/recovery gating,
reserved defaults and staged order. A new
real-core integration covers global/profile order, active refresh, detach fallback,
stopped edits, nodes, committed-runtime restart and failed global regeneration.
Existing authenticated API tests and all six Chromium workflows pass against the
fresh target/mihomo-server-linux-x86_64-globals bundle. Production frontend and
Rust release builds, frontend formatting and all bundle SHA-256 checksums pass.
Catalog assertions include reserved rows, resolve linked merges by saved UID,
and wait for asynchronous node cleanup before testing subsequent failure rollback.
No owned test service/core/worker processes remain after verification.

The Linux MVP remains runnable. Global edits are not yet exposed through the API
or browser: next Delivery step 7 subtask is transactional global Merge/Script
editing with active-profile validation, rollback and interruption recovery, then
its browser editor. Authoritative settings, DNS/TUN, final group cleanup, raw
editing/cascade deletion, scheduling, proxy/TLS modes, upgrades/backups/WebDAV,
advanced pages and additional platforms remain pending. The full project is not
complete.

## Previous increment: transactional global Merge/Script commands

Delivery step 7 adds authenticated global_merge, set_global_merge { yaml },
reset_global_merge, global_script, set_global_script { source }, and
reset_global_script commands. Read returns the reserved UID and retained raw
YAML/source; writes return the updated reserved row. UID Merge/Script stays fixed,
while each save/reset publishes a new private immutable file and timestamp.
Reset restores upstream templates rather than deleting a row or disabling globals.
Base raw content, UID/options/metadata/node records and other global rows are preserved.

The shared profile-merge.yaml journal retains schema version 1 and adds a default
false global flag, preserving previous linked journals. Global records use reserved
file-pointer markers instead of base option links. Recovery accepts the candidate
only when its bound runtime revision committed, or, without an active profile,
when its catalog pointer published. Startup recovers before initializing defaults.
Journal validation rejects wrong global kind/UID/type, unsafe files, nested options,
and changes to unrelated catalog rows; initial creation with no prior row works.
Old immutable global files remain pending garbage collection.

With an active UID, editing either global stage regenerates it even if it has
explicit profile enhancements: globals always precede profile stages. Proposed
content also replaces explicit reserved links and missing-link fallbacks. Generation
uses the current profile name, bounded workers, private-controller checks, Mihomo -t,
reload/restart fallback and coordinated catalog/runtime commit. Validation, script,
publication or core startup failure rolls back saved pointers/runtime/core as
appropriate. Node restoration remains serialized and stopped edits stay stopped.
Broken/missing old source content can be repaired with a valid replacement without
executing or overwriting it; invalid catalog row types remain explicit errors.

Without an active profile, global edits publish catalog/source only and leave a
running standalone configuration, PID and runtime revision unchanged. Merge input
is size/mapping checked and normalized through the real overlay helper before
private-controller validation. Custom scripts are syntax checked in the bounded
worker by parsing the exact execution program without running top-level statements
or main. Runtime behavior, return shape, main presence and Mihomo validity are
checked on future selection; no synthetic profile is supplied. Custom script
validation/execution currently requires Linux limits. Unknown fields, missing/null
content and oversized sources are rejected; resets accept no content fields.

Verification: workspace check, formatting, Clippy with warnings denied, all 135
regular Rust tests and 38 explicitly enabled integration tests pass. Production
frontend and Rust release builds and all seven Chromium workflows pass against
target/mihomo-server-linux-x86_64-global-edits; all bundle SHA-256 checksums pass.
No owned test service/core/worker processes remain. Five new core/store tests cover
reserved identity and links, active/inactive interrupted commits, first-row creation,
broken-source repair, journal tampering and syntax-only nonexecution. A strict HTTP
test covers all new authentication/shape/read/save/reset paths and standalone/current
behavior. Two real/controlled-core tests cover ordering, refresh, node recovery,
publication/restart failure rollback, stopped resets and shutdown cancellation.
A seventh release-bundle browser workflow exercises global API commands, visible
runtime changes, failure preservation and restart/reset alongside the existing MVP.

The Linux MVP remains runnable. Next Delivery step 7 subtask: browser global
Merge/Script editor, including reset, failure diagnostics and responsive layout.
Authoritative settings, DNS/TUN, final group cleanup, raw editing/cascade deletion,
scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional
platforms remain pending. The full project is not complete.

## Previous increment: browser global Merge/Script editors

Delivery step 7 adds a separately named full-width Global enhancements panel on
the subscription page. It remains available with no base profiles and while the
core is stopped/failed. Global merge and script controls read saved content using
the authenticated actor commands; reserved auxiliary rows remain outside base
subscription cards/counts. Editors preserve exact YAML/JavaScript source and call
the existing transactional set commands. Success closes the editor; reopening
reads persisted content, including after service restart.

Failure keeps the draft visible and reports the server's generation/script/core
validation error through the existing feedback. Script console diagnostics remain
in the bounded Logs page. UTF-8 byte limits are checked before sending: 8 MiB merge
YAML and 1 MiB JavaScript; whitespace-only drafts cannot be submitted. Global edit
entry buttons cannot reopen/switch an active editor, and all mutation controls
honor the shared command lock/busy state. Cancel discards only the local draft.

Reset uses a separate inline confirmation describing the template replacement;
continuing editing cancels confirmation without changing storage. Confirmed reset
calls the backend reset command instead of embedding a browser copy of the template.
A source read failure shows an empty unsaved draft and explicit recovery actions:
retry reading, paste full replacement content, or reset. No save is sent for the
empty draft, and failed reads do not silently invent or persist default content.

The panel explains current-profile validation/application, stopped-core retention,
standalone runtime preservation without an active UID, and upstream fallback that
can execute a global script twice. The editor/form/actions and reset confirmation
wrap on narrow screens. This adapts upstream subscription-page global controls;
desktop file-open actions, Monaco and Tauri dependencies are not introduced.

Verification: `cargo check --workspace`, formatting and warning-free all-target
Clippy pass; 135 regular Rust tests and 38 real-Mihomo integration tests pass with
`/usr/bin/verge-mihomo` v1.19.31. TypeScript/Vite production build and frontend
format checks pass. All eight Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-global-editor` release bundle. The eighth covers
exact reads, cancel/save, runtime and console visibility, retained drafts and
catalog/revision/PID on syntax/execution/core failures, client size rejection,
restart persistence, confirmed/cancelled resets, stopped edits, read-error retry,
no-active-profile deferred execution and restored MVP operation. The 390px
full-page screenshot was inspected and the no-horizontal-overflow assertion
passes. All bundle checksums pass. Test shutdown leaves no fixture service,
Mihomo child or script worker behind.

The Linux MVP remains runnable. Next Delivery step 7 subtask: service-owned settings
model/persistence and authoritative runtime fields, followed by DNS/TUN settings
and final group cleanup. Raw editing/cascade deletion, scheduling, proxy/TLS modes,
upgrades/backups/WebDAV, advanced pages, automatic auxiliary creation and additional
platforms remain pending. The full project is not complete.

## Previous increment: persisted runtime settings foundation

Delivery step 7 adds a strict versioned `settings.yaml` store under the locked
service data directory. Missing files initialize `schema_version: 1` with an
empty `runtime` mapping, preserving the existing MVP. The current typed subset
contains mixed/SOCKS/HTTP/redir/tproxy ports, mode, allow-lan, IPv6, unified-delay
and log-level. Explicit zero disables a listener; false remains an explicit value;
absent/null fields inherit source configuration. Unsupported platform listener
fields, unknown fields/types/enum values, out-of-range ports and schema versions
are rejected. Loading rejects symlinks, directories and files over 64 KiB.
Replacement uses a private 0600 temporary file, fsync, rename and directory sync;
rename is the logical commit point. Parse failures do not overwrite existing data.

The lifecycle actor loads one startup snapshot. Generation now exposes a separate
global merge stage so settings enter after rules/proxies/groups and before global
merge/script and profile merge/script. Explicit settings are restored at the end,
following upstream final authority enforcement; scripts can still observe values
from preceding manual stages. Candidate staging enforces them for bootstrap,
imports, raw runtime edits and overlays as well. Discarded overrides enter bounded
logs under `settings`. Unconfigured fields retain existing behavior. Controller
boundary checks still reject attempts to expose a controller; management listener,
tokens, controller paths/secrets, DNS/TUN and desktop settings are not exposed by
this model. Raw subscription files remain unchanged, and settings are not rewritten
by configuration operations.

This is an offline/startup foundation, not an online settings mutation API. Edit
`settings.yaml` only while the service is stopped. Restart loads the new settings
snapshot but starts the last committed runtime without regeneration or script
reexecution. Reapply a subscription or save/import configuration to use newly
loaded settings; initial bootstrap uses settings immediately. This preserves
committed-snapshot recovery, including settings changes between service runs.
Invalid settings currently reject service startup; repair the file offline.

Verification: `cargo check --workspace`, formatting and warning-free all-target
Clippy pass. All 141 regular Rust tests and 39 real-Mihomo integration tests pass
with `/usr/bin/verge-mihomo` v1.19.31. Five new storage tests cover persistence,
strict/unsafe input rejection, explicit authority and write-failure behavior; a
service test checks invalid-settings startup rejection and directory-lock release.
The new real-core workflow covers bootstrap and settings visibility before scripts,
global/profile merge/script overrides, removed and uppercase fields, raw source
preservation, validation failure rollback, stopped overlays, settings persistence
and committed-snapshot restoration. That workflow passes again after adding the
pre-script visibility assertion. TypeScript/Vite production build passes; all eight
Chromium workflows pass against `target/mihomo-server-linux-x86_64-settings`,
confirming default-empty compatibility and retained MVP behavior. All bundle
checksums pass. No fixture service, Mihomo child or script worker remains.

Next subtask: actor-owned online settings read/update commands with coordinated
settings/runtime commit, rollback and interrupted-update recovery. Then extend
DNS/TUN authority and final group cleanup. Service settings UI, raw editing/cascade
deletion, scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and
additional platforms remain pending. The Linux MVP is retained; full completion
has not been reached.

## Previous increment: actor-owned online settings transactions

Delivery step 7 adds authenticated `settings {}` and `set_settings {runtime}`
commands. Both go through the lifecycle actor. The update replaces the entire
typed runtime settings subset, not a patch; omitted/null fields relinquish explicit
authority, and `{}` clears the subset. Schema version remains service-owned.
Unknown envelope/settings fields, missing/null runtime objects and invalid types,
ports or enum values are rejected. Read responses and accepted updates return the
versioned ServiceSettings model. Identical updates are no-ops.

With an active UID, changed settings regenerate the original subscription through
the bounded enhancement pipeline; scripts see new settings before global stages,
then final authority enforcement restores explicit settings. Without an active UID,
the candidate is based on the committed standalone runtime. Removing authority in
that case retains its current field value until a later manual edit/import;
original standalone source values cannot be reconstructed. Both paths perform
Mihomo validation and use existing apply/reload/restart rollback. Stopped cores
remain stopped. With no committed runtime, updates only atomically persist settings
for later bootstrap and do not spawn a core or validator.

After validated runtime staging, a private settings-transaction.yaml journal records
previous/candidate settings and the candidate runtime revision. Core application
precedes settings publication; the runtime manifest commit decides acceptance of
both. Apply/publication/commit failure restores the previous runtime and resolves
settings using that manifest. Startup and command admission recover any surviving
journal before accepting more Actor work. Uncommitted publication is reverted; committed
but unpublished settings are completed. Conflicting or malformed/unsafe journals
fail recovery without silently overwriting administrator edits; recovery checks the
actual settings file as well as the journal. Failed recovery retains the journal
and gates Actor commands until repaired, while status/log snapshots remain readable. Publication and
cleanup errors remain observable. In-memory settings follow the persisted snapshot,
including failures after a logical commit. No Web settings page/event is added yet.

Verification: `cargo check --workspace`, formatting and warning-free all-target
Clippy pass. All 145 regular Rust tests and 40 real-Mihomo integration tests pass
with `/usr/bin/verge-mihomo` v1.19.31. Storage and actor recovery tests exercise
interrupted publication, accepted/rejected revisions and actual-file conflict
rejection. Authenticated HTTP tests check envelope/type/auth errors and no-runtime
save/clear. The new real-core workflow checks standalone and active application,
script input, relinquished authority, generation/validation/publication failures,
recovery gating until a damaged settings file is repaired, stopped retention and
restart persistence. The global-profile fixture now includes an atomic directory
counter and exclusive directory creation after a parallel regression exposed
temporary-directory isolation failures. Frontend format and TypeScript/Vite
production build pass. All nine Chromium workflows pass against the corrected
`target/mihomo-server-linux-x86_64-online-settings-final` release bundle, including
HTTP settings save/clear, active UID preservation, reopened configuration view,
stopped-state retention and service restart. All bundle checksums pass; no fixture
service, Mihomo child or script worker remains.

Next subtask: a browser service-settings editor using the read/replace commands,
explicit inheritance semantics and failure-retained drafts. Then extend DNS/TUN
authority and final group cleanup. Full settings/resources, raw editing/cascade
deletion, scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and
additional platforms remain pending; the project is not fully complete.

## Previous increment: browser runtime settings editor

Delivery step 7 adds a `/settings` navigation entry and a separate responsive
runtime-settings editor. The scoped Rust SPA navigation allowlist also serves
direct GET/HEAD `/settings` navigation without widening API/asset fallbacks.
It reads all ten currently supported fields before enabling
the form: mixed/SOCKS/HTTP/redir/tproxy ports, mode, allow-lan, IPv6, unified-delay
and log-level. Blank ports and an explicit Inherit selection omit authority; zero
and false remain explicit values. Every save submits the full subset, retaining
all unmodified fields. Ports must be decimal integers 0–65535; client validation
rejects invalid drafts without sending a mutation. Platform restrictions remain
backend-validated and are explained beside the form.

A separate summary shows the last read saved settings, rather than conflating
inherited values with effective runtime values. The page explains active
regeneration, stopped retention, no-configuration persistence, standalone authority
release and leaving-page draft loss. Replacing a dirty draft with a fresh read uses
confirmation; resetting all fields to Inherit also uses confirmation and only
changes the draft until saved. Failed reads never create a writable default form.
Unsupported schemas/fields/types cannot silently become a subset replacement.

Mutations use the existing authenticated shared command lock. After any save
outcome, an independent authenticated settings read reconciles the result without
clearing the original backend error. Matching accepted content is confirmed;
failed saves keep the draft, including requests reporting an error after the
service actually committed. A failed verification blocks further saves until the
user checks saved settings or reloads. Manual verification preserves the draft;
reload replaces it only after a successful supported read. Reads have owned
AbortControllers cancelled on page exit/logout, and read authentication failure
invalidates the session. This adapts the upstream Settings fields without
desktop system-proxy/controller/DNS/TUN controls or a GUI framework dependency.

Verification: `cargo check --workspace`, formatting and warning-free all-target
Clippy pass. All 145 regular Rust tests and 40 real-Mihomo integration tests pass
with `/usr/bin/verge-mihomo` v1.19.31. The static-assets regression includes direct
GET/HEAD `/settings`; TypeScript/Vite build and frontend format checks pass. All ten
Chromium workflows pass against the corrected
`target/mihomo-server-linux-x86_64-settings-editor-final` release bundle. The initial
MVP workflow verifies no-config settings save/clear before repairing startup. The
tenth checks all fields, full replacement, inheritance/zero/false, client limits,
reload confirmation, real Mihomo rejection with preserved PID/revision/draft,
failed post-save read, actual commit with a lost response, confirmed clear,
standalone release behavior, stopped retention, restart persistence and unsupported
response protection with a dirty draft. Desktop and 390px full-page screenshots
were inspected; the mobile no-horizontal-overflow assertion passes. All bundle
checksums pass. Test shutdown leaves no fixture service, Mihomo child or script
worker behind. The Linux MVP remains runnable.

Next subtask: extend the service settings model and enhancement pipeline with
DNS/TUN settings and corresponding authority enforcement, followed by final group
cleanup. Full settings/resources, raw editing/cascade deletion, scheduling,
proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional platforms
remain pending. The project is not fully complete.

## Previous increment: typed DNS/TUN settings and field authority

Delivery step 7 extends the versioned runtime settings subset with strict optional
DNS and TUN objects. `config/settings/network.rs` models basic DNS fields and the
upstream saved TUN dialog keys with typed enhanced-mode/stack enums. Unknown
nested fields/types/enums fail decoding; TUN MTU must be 1–65535, device nonempty
and auto-redirect is Linux-only. Defaults remain empty, preserving existing MVP
configuration, schema version and transaction formats. The 64 KiB settings limit
also bounds these objects.

Settings apply shallowly by owned DNS/TUN key, retaining subscription/custom
fields. DNS follows upstream is_set: false, null, blank strings and empty lists
inherit; true/nonempty fields have authority. TUN false and empty lists remain
explicit overrides. Removing or replacing a section in manual enhancements cannot
remove owned fields. Settings enter before global/profile merges and scripts and
are restored after them and at candidate staging, including standalone imports,
edits and overlays. Bounded diagnostics now report paths such as dns.nameserver
and tun.enable instead of attributing the whole section. Unowned provider policies
and custom fields retain their previous values.

The authenticated settings read/whole-runtime replacement commands automatically
carry both objects through the existing actor, validation, stopped updates,
settings/runtime commit journal, failure rollback and startup recovery. No-runtime
updates persist for bootstrap. Existing scalar browser controls deliberately refuse
nested settings and cannot silently discard them; the API is the current editing
path. Documentation records both the replacement contract and the DNS/TUN
inheritance difference.

Verification: `cargo check --workspace`, formatting and warning-free all-target
Clippy pass. All 147 regular Rust tests and 41 real-Mihomo integration tests pass
with `/usr/bin/verge-mihomo` v1.19.31. TypeScript/Vite production build and frontend
format checks pass. All eleven Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-network-settings` release bundle; all bundle
checksums pass and its provenance matches the current source document.
Regression coverage adds
strict nested decoding/API round trips, shallow preservation/removed-map recovery,
DNS inactive values versus explicit TUN false/empty lists, nested journal recovery,
real-core enhancement authority, rejected malformed resolver URLs with unchanged
settings/runtime/PID, stopped updates and restart. A release-bundle browser workflow
checks real nested snapshots, runtime/restart persistence and scalar-editor protection.
Integration tests keep TUN disabled and do not establish privileged native TUN
routing or automatic DNS derivation. Test shutdown leaves no fixture service,
Mihomo child or script worker behind.

Next Delivery step 7 subtask: migrate pure TUN-to-DNS derivation and provider DNS
conflict/confirmation semantics, then expose dedicated DNS/TUN editing and finish
group/LAN cleanup. Hosts/fallback-filter/policy settings, full resource settings,
raw editing/cascade deletion, scheduling, proxy/TLS modes, upgrades/backups/WebDAV,
advanced pages and additional platforms remain pending. The Linux MVP remains
runnable, but the complete project is not finished.

## Previous increment: pure TUN/DNS derivation and single-pass candidate phases

Delivery step 7 migrates the pure upstream TUN stage into
`headless-core/enhance/tun.rs`. Explicit TUN enable derives fake-IP DNS enable and
IPv6 from the effective top-level settings, adds missing fake-IP mode/IPv4 range
and, with IPv6, its default range. Existing ranges and redir-host DNS survive;
disabled TUN only changes its enable field. Absent/null tun.enable retains
headless inheritance and does not implicitly take control of a source-owned TUN.
The optional DNS settings stage afterward also repairs missing/null/blank or
non-string IPv6 ranges for fake-IP DNS. Host public-DNS changes and privilege
management from desktop code are not included.

RuntimeSettings::prepare now applies ordinary/TUN fields → pure TUN derivation →
DNS fields/range repair before global/profile enhancements. Derived DNS values
do not automatically become authoritative settings; late merges/scripts can
change them unless the DNS page owns that specific field. Final enforcement
remains restricted to explicit owned fields.

The lifecycle actor distinguishes raw and already enhanced candidates privately.
Bootstrap, standalone import/edit/overlay and standalone settings updates prepare
raw inputs once. Profile selection, active refresh, linked/global changes and
active settings updates carry enhanced candidates to validation without deriving
again after scripts. No new API/YAML marker, schema version or transaction format
is introduced. Existing validated apply, rollback, settings publication, stopped
behavior and committed-snapshot recovery remain shared. Startup restoration does
not regenerate a committed runtime or rerun scripts.

Verification: all 153 regular Rust tests and 42 real-Mihomo integration tests pass
with v1.19.31. Six new pure tests cover enabled/disabled TUN, redir-host, default
ranges, malformed maps and present-null behavior, post-DNS IPv6 repair,
settings phase order and inheritance. A real-core test validates enabled TUN only
while stopped, checks script-visible derived inputs, unchanged late script output,
explicit TUN authority, invalid resolver rollback, unchanged raw subscriptions,
committed-snapshot restoration and a safe start after disabling TUN.
`cargo check --workspace`, Rust formatting and warning-free all-target Clippy pass.
TypeScript/Vite production build and frontend format checks pass. All eleven
Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-tun-derivation` release bundle. Its checksums pass
and bundled provenance matches the source document. Test shutdown leaves no fixture
service, Mihomo child or script worker behind. Privileged native
TUN routing is not established by these generation/`mihomo -t` checks.

Next Delivery step 7 subtask: provider-specific DNS conflict detection and scoped
confirmation/auto-disable semantics with transaction-safe persistence, then the
dedicated DNS/TUN editor and final group/LAN cleanup. Hosts/fallback-filter/policy
settings, full resource settings, raw editing/cascade deletion, scheduling,
proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional platforms
remain pending. The Linux MVP remains runnable; the complete project is not done.

## Previous increment: provider DNS confirmation and coordinated auto-disable

Delivery step 7 migrates provider-specific DNS source detection and per-profile
preferences. The raw subscription's nonempty proxy-server-nameserver,
proxy-server-nameserver-policy and nameserver-policy fields are canonically hashed
with UID using the upstream SHA-256 format before manual enhancement stages.
Absent/empty/ordinary DNS fields do not create conflicts. Formatting/key order
does not invalidate confirmation; another UID or changed resolver/policy does.
Ring reuses the already pinned service dependency and a fixed compatibility
digest verifies the extraction.

Versioned settings now optionally store strict profile_dns enabled flags. Existing
empty/default settings remain compatible; whole-runtime set_settings preserves
preferences. Confirmation digests live only in the actor session and never appear
in settings/journals. They survive profile switches, expire on restart/source
changes, and are restored when a tentative confirmation fails before commit.
The authenticated profile_dns read reports source/requested/enabled; set_profile_dns
requires the active UID, returns confirmation_required without mutation for a
missing/stale digest, or applies validated enable/disable. It requires saved DNS
settings before enabling. Dedicated browser controls are still pending.

Unconfirmed generation suppresses DNS-page values and authority, preserving
independent TUN derivation and manual enhancement behavior. Only the affected
profile's preference becomes disabled. Raw/enhanced candidates carry the decision
through final authority and validation. Automatic preference changes join the
settings/runtime journal even when the operation also has a refresh/enhancement
journal. Runtime manifest commit decides all publication/recovery, and only
committed changes invalidate cached confirmation or emit bounded diagnostics.
Inactive enhancement checks do not change preferences.

Startup still restores committed YAML without scripts or regeneration. Session
confirmation expiry protects the next reselect/refresh/edit; it does not rewrite
an already committed snapshot. This headless recovery adaptation is explicit in
RUNNING.md. Deleted-profile preference cleanup is deferred with broader cascade
deletion; UID map keys remain passive preferences and cannot select file paths.

Verification: all 157 regular Rust tests and 44 real-Mihomo integration tests pass
with v1.19.31. `cargo check --workspace`, Rust formatting and warning-free all-target
Clippy pass. Added coverage includes
canonical/source/session behavior and strict preferences, nested preference journal
recovery, authenticated command decoding, inactive mutation rejection, active
confirmation/mismatch/switch/restart, failed validation without leaked confirmation,
and failure after refresh publication with settings-journal recovery. A twelfth
release-bundle browser workflow exercises actual confirmation responses, runtime
content, restart expiry and preservation of preferences by the scalar editor.
TypeScript/Vite production build and frontend format checks pass. All twelve
Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-dns-confirmation` release bundle. Its checksums
pass and bundled provenance matches the source document. The final six settings
integration workflows also pass after the lifecycle exit-observation guard was
added. Test shutdown leaves no fixture service, Mihomo child or script worker.
The DNS fallback
validation fixture disables GeoIP fallback explicitly to avoid implicit external
resource downloads; diagnostic contexts identify the enhancement stage on failure.

Next Delivery step 7 subtask: dedicated DNS/TUN settings and conflict-confirmation
editor, implemented below, followed by final group/LAN cleanup. Policy/hosts/fallback-filter settings,
native TUN integration, full resources, raw editing/cascade deletion, scheduling,
proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional platforms
remain pending. The Linux MVP remains runnable; the complete project is not done.

## Previous increment: DNS/TUN editor and explicit provider confirmation

Delivery step 7 extends the existing `/settings` page with every currently typed
DNS/TUN field. Scalar and network values share whole-runtime replacement,
validation, failed-draft retention and independent readback. Separate section and
string inheritance controls preserve absent values, empty section objects and
empty DNS strings; JSON string-array inputs preserve arbitrary delimiters and
escaped newlines. DNS false/empty values remain saved but do not gain authority;
TUN false and empty lists are explicit. MTU checks match the backend. Unknown
nested fields fail closed while retaining the existing draft. The saved network
snapshot shows last-read values, not inferred runtime behavior.

An independent active-profile panel reads saved preference, generation permission
and provider-source conflict, and calls the existing actor mutation. A conflict
opens an explicit UID/source question; cancel has no side effects. Draft changes,
active UID changes, runtime generation/revision changes and disconnection discard
old questions. The panel independently reads back even failed mutation responses,
blocks uncertain resubmission and never auto-confirms a changed digest. Owned
reads are cancelled on navigation and authentication expiry logs out.

Release-browser testing exposed a restart reconciliation bug: an unchanged
committed revision and coincidentally identical generation could retain old
permission in the page. The panel now observes management WebSocket connection
changes, invalidates questions while disconnected and rereads permission on
reconnection. The UI explicitly distinguishes expired session permission from an
unchanged committed runtime snapshot. Disabling an existing preference remains
available without a DNS section; enabling requires saved DNS settings.

Verification: `cargo check --workspace`, all 157 regular Rust tests, all 44
real-Mihomo v1.19.31 integration tests, Rust formatting and warning-free all-target
Clippy pass. TypeScript/Vite production build and frontend formatting pass. All
13 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-network-editor` release bundle. The new workflow
covers full supported-field preservation, false/empty values, section removal,
MTU/list errors, real core validation failure retaining the draft and revision,
confirmation/cancel, changed raw source during an open question, refresh,
restart/reconnect expiry, UID switches and unknown nested schemas. Existing
workflows continue to cover uncertain responses, initial read failures and
inheritance/reload confirmation. Desktop and 390px mobile screenshots are reviewed
and mobile overflow is checked. Bundle checksums and provenance are verified;
fixture shutdown leaves no service, Mihomo child or script worker.

Next Delivery step 7 subtask: final proxy-group cleanup and LAN-related generation
normalization, implemented below with upstream ordering and settings authority retained. Full DNS
policy/hosts/fallback-filter settings, privileged native TUN integration,
deleted-profile preference cleanup, resources, raw editing/cascade deletion,
scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional
platforms remain pending. The Linux MVP remains runnable; the complete project
is not done.

## Previous increment: final proxy-group cleanup and LAN normalization

Delivery step 7 extracts upstream is_loopback_bind_address,
is_ipv4_shorthand_loopback, ensure_lan_bind_address and cleanup_proxy_groups into
a pure finalize module, composing the existing use_sort. Temporary sets use
standard String instead of smartstring with identical membership behavior.
After authoritative runtime settings are restored, enabled allow-lan widens only
an explicitly loopback bind-address to `*`. Localhost, loopback IPv4/IPv6,
bracketed IPv6 and bounded IPv4 shorthand match upstream. Missing/custom bindings,
disabled allow-lan and malformed values are preserved. Management HTTP binding
and the private core controller are unaffected.

Group cleanup removes unknown/nonstring provider references. With a surviving
valid provider, dynamic string proxy names remain; otherwise only known proxies,
groups, providers and upstream built-ins remain. Order, duplicates and nonstring
proxy entries are preserved. Empty groups get no invented DIRECT fallback;
malformed shapes and rules remain for Mihomo validation. Sorting retains upstream
control/custom/bulky-field order.

Finalization is wired into the sole candidate staging boundary after all manual
merges/scripts and final authority, before store staging and `mihomo -t`.
Bootstrap, standalone YAML/overlay, selection, active refresh, settings and active
linked/global updates share it. Intermediate enhancement mappings are not
cleaned, allowing late scripts to create referenced nodes. Raw/imported/downloaded
files are unchanged. Startup restores committed snapshots without scripts,
normalization or sorting again. Existing publication and rollback journals are
unchanged; invalid finalized candidates do not publish links/settings/runtime.

Verification: all 162 regular Rust tests and 46 real-Mihomo v1.19.31 integration
tests pass, including five new pure cases and two new integration workflows.
Coverage includes upstream LAN/group/provider semantics, malformed values,
ordering/idempotence, bootstrap, standalone/overlay, stopped application, late
script-created references, restored settings authority, failed core validation
preserving PID/revision/script content and committed snapshot recovery despite
offline settings/script edits. `cargo check --workspace`, formatting and
warning-free all-target Clippy pass. TypeScript/Vite release build and frontend
formatting pass. All 14 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-final-cleanup` release bundle. The new browser
workflow verifies active remote refresh/source preservation, LAN settings and
runtime YAML preview, actual core group contents and restart persistence. Bundle
checksums pass and bundled provenance matches source. Fixture shutdown leaves no
service, Mihomo child or script worker.

Next Delivery step 7 subtask: original subscription YAML editing, implemented
below with bounded inputs, active validation/application and raw/runtime recovery;
then auxiliary cascade deletion and stale DNS preference cleanup. Complete DNS
policy/hosts/fallback-filter settings, native TUN integration, resources,
scheduling, proxy/TLS modes, upgrades/backups/WebDAV, advanced pages and additional
platforms remain pending. The Linux MVP remains runnable; the complete project
is not done.

## Previous increment: original subscription YAML read/edit and recovery

Delivery step 7 adds authenticated profile_raw/set_profile_raw commands and actor
read/edit operations for local/remote base profiles. Reads return exact uid,
opaque file revision and source text through existing safe bounded-file checks.
Writes compare that previously read revision, preserve all serialized metadata
except the immutable file pointer, and preserve exact raw text/comments/BOM/line
endings. Reserved/auxiliary items remain on typed editors. A changed revision
rejects stale writes before validation/publication and supersedes in-flight
remote downloads through the existing file-pointer guard.

Following upstream save_profile_file, the service first parses and validates
original YAML with Mihomo, even for inactive profiles. The private controller
boundary remains enforced. Active edits then run saved enhancements, DNS decision,
settings authority and final cleanup, validating/applying the generated candidate.
Both validations must pass. Inactive edits do not execute linked scripts or change
the active runtime/PID/revision. Stopped active edits remain stopped. Original
validation copies use existing immutable runtime staging without a manifest
mutation; staging/source-file garbage collection remains pending.

The existing profile-refresh journal adds optional kind: raw_edit. Legacy refresh
journals retain their unchanged format and default kind. Raw journals reject
metadata changes and unsafe content pointers. Shared admission/startup/rollback
recovery follows committed runtime revision for active edits and catalog pointer
for inactive edits. Changed provider DNS policy invalidates permission and persists
its disabled preference in the same raw/runtime/settings commit; failed candidate
publication restores the previous source, settings and confirmation.

The Profiles page now has an Original YAML editor. Failed initial reads cannot
produce a replacement draft. Independent readback reconciles success/errors/lost
responses, retains failed drafts and blocks uncertain resubmission. Catalog or
readback version changes require explicit reload before an old draft can write
the new base; verification does not silently adopt a changed revision. Dirty
reload/close asks to discard explicitly, owned reads cancel on unmount, deleted
profiles remove the editor and authentication expiry logs out. Remote refresh
intentionally replaces manual raw changes.

Verification: all 168 regular Rust tests pass, including five new raw-store tests
and strict authenticated command coverage. Two new real-Mihomo workflows verify
active/inactive validation, enhanced-only validation failures, stale edits,
metadata/content preservation, stopped updates, restart and simultaneous DNS
preference publication failure/recovery. All 48 real-Mihomo integration tests
pass. All 15 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-raw-editor` release bundle. The added workflow
covers initial read failure, raw/enhanced validation errors, metadata preservation,
stopped save, lost mutation response, failed readback, remote refresh conflict,
explicit draft reload/discard, inactive editing and restart persistence. Desktop
and 390px mobile editor screenshots are reviewed and page overflow is checked.
Bundle checksums and provenance match; fixture processes are fully cleaned up. A pre-existing startup assertion exposed an
asynchronous group snapshot race; it now waits within a bounded deadline for
restored selection rather than indexing a possibly absent group. Production
readiness/restoration behavior is unchanged. `cargo check --workspace`, Rust
formatting, warning-free Clippy and TypeScript/Vite build pass.

Next Delivery step 7 subtask: auxiliary cascade deletion with shared/reserved link
protection and cleanup of deleted-profile DNS preferences. Full DNS policy/hosts/
fallback-filter settings, privileged native TUN, resources, scheduling, proxy/TLS
modes, upgrades/backups/WebDAV, advanced pages, revision garbage collection and
additional platforms remain pending. The Linux MVP remains runnable; the complete
project is not done.

## Previous increment: auxiliary cascade deletion and DNS preference cleanup

Delivery step 7 extends noncurrent local/remote deletion to exclusive linked
merge/script/rules/proxies/groups auxiliary rows. One catalog rename removes the
base and planned auxiliaries. Shared links, reserved Merge/Script/Rules/Proxies/
Groups rows and any files referenced by surviving catalog rows are protected.
Active/current deletion remains rejected even when stopped; there is no implicit
fallback selection. Missing auxiliary rows/files are tolerated for repair, while
wrong-type/nested auxiliaries and incoming links to the base are rejected.

The private schema-2 deletion journal records up to five auxiliary UID/file pairs;
schema-1 plans still recover. Publication checks the staged plan against current
links and file pointers. Uncommitted recovery preserves rows, content and DNS
preferences. Committed recovery atomically removes the deleted preference before
cleaning unreferenced files, and retains the journal through any cleanup failure.
Startup and command admission retry settings/file cleanup. Existing runtime
settings, revision, active UID and running PID are unchanged. Actor confirmations
are removed only after catalog commit. Startup also prunes preferences left by
older deletions after recovering all relevant journals.

The Profiles page confirmation describes removal of exclusive auxiliary configs
and DNS preferences, and preservation of shared configurations. Existing readback
and profile watches report committed deletion even when cleanup must be retried.
Garbage collection of old immutable source/runtime/auxiliary revisions and unlinked
orphan files remains a separate pending task.

Verification: all 179 regular Rust tests pass, including nine new store tests for
five-kind cascade, shared/reserved protection, catalog/settings write failures,
interruption recovery, partial cleanup, forged plans, legacy journals, stale
preferences and symlink/path safety. All 52 real-Mihomo integration tests pass;
the two added actor workflows also pass after extending coverage to an enabled
provider DNS preference/session confirmation. They verify unchanged runtime/PID,
active/stopped protection, recovery before the next command and restart persistence.
All 17 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-cascade-delete` release bundle, including the
new confirmed cascade/file/preference/restart workflow. Cargo check, Rust format,
warning-free Clippy, TypeScript/Vite and changed-file Prettier checks pass. The
full frontend formatting check still reports pre-existing style differences in
unmodified proxy-access.tsx, settings.tsx and style.css. Bundle checksums and
provenance match, and fixture processes have been cleaned up.

Actual-node smoke verification uses an isolated copy of the existing data catalog,
source files and GeoIP data. It loads the saved subscription with 58 source nodes
(56 traffic candidates), selects a real node through the management API, confirms
the selected node via Mihomo readback, and receives HTTP 204 from an HTTPS request
through the generated HTTP proxy listener on the first candidate. Original data
files remain byte-for-byte unchanged; no credentials or node endpoints are printed.

Commit status: the worktree changes and progress documents are complete, but
`git add` fails creating `.git/index.lock` because this environment mounts `.git`
as a read-only filesystem. No commit was created; the changes remain in the
worktree for submission when Git metadata is writable.

Next Delivery step 7 subtask: automatic per-profile auxiliary creation with upstream
defaults and safe import/recovery. Full DNS policy/hosts/fallback-filter settings,
privileged native TUN, resource settings, scheduled/proxy/TLS updates, core upgrades,
backups/WebDAV, advanced pages, revision garbage collection and additional platforms
remain pending. The Linux MVP remains runnable; the complete project is not done.

## Previous increment: automatic import auxiliary defaults and recovery

Delivery step 7 now routes service local-file/YAML, remote and CLI imports through
one workflow that creates missing ordinary merge/script/rules/proxies/groups rows
using upstream defaults. Every profile owns distinct initial auxiliaries. Supplied
valid shared/reserved links and remote metadata are preserved. Empty Merge does
not contain the reserved global store-selected setting; Script is the upstream
identity template. New profiles execute global Script once followed by their
identity Script. Cleared or legacy missing links retain reserved fallback.

A private bounded profile-import.yaml journal records allocated UID/file pairs,
content hashes and reused-link fingerprints before content writes. One atomic
catalog rename publishes all rows. Startup and command admission recover imports
before global initialization and other mutations: committed exact rows/content
survive, uncommitted partial files are removed, reused files remain, and unsafe or
conflicting plans fail with retryable recovery state. Imports preserve exact local
raw text, remain separate from activation and leave active UID/runtime/PID unchanged.
Refresh/raw/metadata updates keep saved links; cascade deletion removes their
exclusive current files. Existing catalogs retain their prior link/fallback behavior.
Low-level raw import primitives remain available for legacy catalog migration;
service entrypoints use the new transactional default-import methods.

Verification: all 188 regular Rust tests and all 54 real-Mihomo integration tests
pass. Nine new store tests cover exact templates/raw content, separate ownership,
shared/reserved reuse, remote metadata, partial-file interruption, failed catalog
writes, filename collisions, changed reused links, pending admission, forged plans,
corrupt content, partial catalogs and symlink-safe retry. Two new actor workflows
verify unchanged core/runtime on import, once-only global execution, explicit
clear/fallback, startup recovery before global initialization and restart behavior.
Existing service tests now assert preserved initial links and the new catalog
shape while retaining clear, failed-update rollback and legacy fallback coverage.

All 18 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-import-defaults` bundle. The new workflow verifies
local/remote owned templates, distinct UIDs, unchanged runtime/PID, refresh link
preservation, initial editor content and restart persistence. Existing enhancement
workflows now wait for save completion instead of using an already-present link
indicator, and verify the full upstream sequence templates. Cargo check, Rust
format, warning-free Clippy, TypeScript/Vite and changed-test Prettier checks pass.
Bundle checksums and bundled provenance/deployment docs match the source. Fixture
services, cores and script workers are cleaned up.

Actual-node verification imports the existing saved raw node data through the new
service workflow in an isolated temporary data directory. All five auxiliary links
are created, the selected real node is confirmed through Mihomo readback, and an
HTTPS request through the generated HTTP proxy returns 204 on the first candidate.
There are 58 source nodes and 56 traffic candidates. Original data files remain
byte-for-byte unchanged and no node credentials or management token are printed.

Commit status: `git add` again fails creating `.git/index.lock` because Git metadata
is mounted read-only. No commit was created; the verified worktree changes remain
available. The pre-existing run_autonomous_codex.sh modification was not changed or
included in the staging request.

Next Delivery step 7 subtask: remote subscription downloads through the managed
core proxy (self_proxy), preserving bounded download/admission, cancellation,
refresh stale guards and recovery. System proxy/TLS modes, scheduled updates,
full DNS/hosts/TUN settings, resources, core upgrades, backups/WebDAV, advanced pages,
revision garbage collection and additional platforms remain pending. The Linux
MVP remains runnable; the complete project is not done.

## Previous increment: managed-core proxy subscription downloads

Delivery step 7 now supports self_proxy on remote import, saved remote metadata
patches and manual refresh. Missing/false retains direct transport. Persisted
upstream self_proxy wins over with_proxy; system proxy and invalid-certificate
bypass remain explicitly unsupported. UI import/edit checkboxes save the mode,
keep failed import drafts and allow an explicit return to direct mode.

After semaphore admission the service reads the private controller and committed
runtime under a stable running PID/generation/configuration snapshot. A bounded
three-second route query verifies actual listener ports against the committed
configuration, prefers Mixed then HTTP, and supports loopback IPv4/IPv6 bindings.
Missing ingress, stopped cores, incompatible custom bindings or route/auth changes
fail explicitly without direct fallback. Mihomo reports usernames only; credentials
come from the private committed runtime and the reported usernames are checked.
The opaque route is neither serialized nor logged; no proxy URL is caller supplied.

Network work remains outside the lifecycle actor. Existing size/concurrency,
timeout/redirect/TLS limits, URL-redacted errors and shutdown cancellation remain.
Explicit routing overrides environment proxy/NO_PROXY choices. Core stop/restart,
child replacement or committed configuration change cancels pending proxy downloads
and releases admission. Existing source UID/file/URL/full-option guards prevent
stale refresh publication, including download-mode changes. Successful downloads
use the original recoverable import/defaults and refresh transactions.

Verification: 191 regular Rust tests and 56 real-Mihomo integration tests pass.
Three route/validation tests and two real-core workflows cover listener priority,
loopback bindings, authentication mismatch/redaction, private password use, a
controlled HTTP upstream tunnel to a synthetic hostname unavailable directly,
Mixed-to-HTTP port switching, response-size/status/timeout failures, mode/restart persistence,
refresh metadata conflicts, stop/config-change cancellation and absent ingress.
Provider/upstream headers contain no management or inbound proxy credentials.
Cargo check, Rust format, warning-free all-target Clippy and TypeScript/Vite pass.
All 19 Chromium workflows pass against the fresh
`target/mihomo-server-linux-x86_64-self-proxy` release bundle. The new workflow
verifies proxy import, saved mode/restart persistence, stopped-core rejection with
no provider request, retained URL/name/mode drafts, metadata switch to direct and
successful stopped-core direct refresh/import. Changed-file Prettier, bundle
checksums and bundled provenance/deployment document comparisons pass. Fixture
services, cores and script workers are cleaned up.

Actual-node verification again imports the saved source into an isolated temporary
service, confirms all five auxiliary defaults and real selected-node readback, and
gets HTTPS 204 through the proxy on the first candidate among 56 traffic nodes.
Original data remains byte-for-byte unchanged; secrets are never printed.

Commit status: `git add crates service web docs` fails creating .git/index.lock
because Git metadata is mounted read-only. No commit was created; the completed
worktree changes remain available. The pre-existing run_autonomous_codex.sh
modification remains untouched and was excluded from the staging request.

Next Delivery step 7 subtask: system-proxy remote subscription transport
(with_proxy), with explicit server semantics and the same bounds/cancellation/
refresh recovery. TLS fallback/bypass, scheduled updates, full DNS/hosts/TUN settings,
resources, upgrades, backups/WebDAV, advanced pages, revision garbage collection
and additional platforms remain pending. The Linux MVP remains runnable; the
complete project is not done.

## Previous increment: service system-proxy subscription downloads

Delivery step 7 now supports with_proxy on import, strict saved metadata patches
and manual refresh. Only with_proxy true and self_proxy false enables system
discovery; direct/default and managed modes still disable implicit proxies.
Upstream self_proxy > with_proxy > direct precedence is retained, including saved
profiles with both flags. Missing/false remains opt-in behavior, and explicit false
patches disable the saved mode without changing other profile options/links.

Linux discovery uses Reqwest 0.13.5 / Hyper-util 0.1.20's service process environment:
HTTP(S)_PROXY and ALL_PROXY, uppercase before lowercase, plus NO_PROXY domain/IP/
CIDR matching. A `*` bypass entry is explicitly global for IP literals too,
correcting the locked matcher's domain-only wildcard. Missing configuration or
bypass permits direct access, matching upstream disabled/unavailable discovery.
CGI REQUEST_METHOD suppresses discovery, following the locked matcher. Native
Windows/macOS discovery remains library code awaiting runtime verification.
No Linux desktop session, PAC/WPAD or external discovery command is required.

Effective variables are bounded UTF-8/control-free values; HTTP(S) endpoint
validation rejects malformed/unsupported routes with only variable names in errors.
Configured connection/status/TLS failures never retry through direct mode. Basic
proxy authentication and HTTPS CONNECT use the library matcher; proxy credentials
are absent from bypassed origin headers after redirect. Proxy endpoints are trusted
deployment environment, never management-supplied URLs or API readback. The
service never changes machine proxy settings or forwards management credentials.

All modes retain download admission, request/body/redirect limits, verified TLS,
shutdown cancellation and existing import/refresh recovery. System downloads run
outside the lifecycle actor and survive unrelated managed-core stop/reload. Changes
to URL/source/metadata/options invalidate an older refresh through existing guards.
UI checkboxes expose import and saved refresh modes, explain service environment
and priority, retain failed drafts, and support explicit switches back to direct.

Verification: 200 regular Rust tests pass. Five environment tests cover valid
endpoints, credentials, malformed/unsupported/oversized/non-Unicode values,
uppercase/empty precedence, CGI suppression and global bypass. Four isolated
service workflows verify actual proxy authentication, protocol/env precedence,
NO_PROXY, redirects and header isolation, no-configuration direct behavior,
HTTPS CONNECT, stale metadata, restart persistence, failed-route non-fallback,
size/time/admission bounds and active/queued shutdown. One real-Mihomo workflow
confirms authenticated managed priority and system refresh surviving core stop.
All 57 real-Mihomo integration tests and all 20 Chromium workflows pass against
the fresh `target/mihomo-server-linux-x86_64-system-proxy-verified` bundle. The new
browser workflow verifies stopped-core system import/refresh, mode persistence,
metadata switch to direct, both-flag managed priority with retained failed drafts,
and explicit retry through system mode. Existing managed/direct workflows also
pass with a controlled proxy in the service environment. Cargo check, Rust format,
warning-free all-target Clippy, TypeScript/Vite, changed-file Prettier, bundle
checksums and bundled provenance/deployment document comparisons pass.

Actual-node smoke uses the saved node data in an isolated temporary service with
HTTP_PROXY/HTTPS_PROXY pointing to its generated proxy and NO_PROXY for management.
The first of 56 traffic candidates returns HTTPS 204 after selected-node readback.
A with_proxy HTTPS download also reaches subscription YAML validation: the empty
204 body is correctly rejected without a catalog change. This verifies HTTPS
transport independently of successful subscription content, which the controlled
proxy workflows cover. Original data remains byte-for-byte unchanged, no secrets
are printed, and fixture services/cores/script workers are cleaned up.

Git handoff: per the user's updated instructions, no Git write or commit is
performed in the sandbox. The external host script owns the completed increment's
commit. Unrelated automation/script edits are left untouched.

Next Delivery step 7 subtask: TLS trust-root fallback and explicit subscription
certificate options, preserving redaction, transport priority, bounds, cancellation
and recovery. SOCKS/PAC, scheduled updates, full DNS/hosts/TUN settings, resources,
upgrades, backups/WebDAV, advanced pages, immutable-file garbage collection and
additional platform runtime/deployment checks remain pending. The Linux MVP remains
runnable; the complete project is not done.

## Previous increment: subscription TLS roots and explicit certificate options

Delivery step 7 now follows upstream platform-first TLS verification with one
static Mozilla/WebPKI root retry for TLS trust/revocation errors. Protocol-version
errors do not retry and report TLS 1.2/1.3. Default verification rejects untrusted
and wrong-hostname certificates; static roots retain both certificate and hostname
verification. Explicit danger_accept_invalid_certs true disables both checks only
for that subscription and its redirects and never triggers a root retry.

The strict import/metadata schemas, upstream PrfOption, saved refresh options and
Web import/editor checkboxes now support the certificate flag. Explicit false
restores verification; omitted/null metadata retains saved values. Editing the
flag during refresh rejects the old result, preserves raw bytes and catalog, and
leaves auxiliary links intact. Values survive service restart. Failed import drafts
remain visible so retry requires an explicit setting change.

Both attempts share one overall download deadline, bounded admission, redirects,
body/YAML/status limits and shutdown cancellation. The managed route/credentials
remain fixed across retry, self_proxy still wins over system discovery and direct,
and TLS failures cannot switch transport mode. Reqwest's public API builds verified
static roots from the already locked DER bundle without older TLS backend injection.
TLS errors omit URLs and credentials while retaining their cause classification.

Verification: local HTTPS fixtures cover default failure/two handshakes, explicit
bypass with wrong hostname, one total timeout, protocol-version alert/no retry,
HTTP/body/size/YAML limits, HTTP-to-HTTPS redirect, platform custom CA, authenticated
system CONNECT across retry, saved edits/restart, stale refresh and cancellation.
Real Mihomo additionally covers authenticated managed HTTPS, priority over system
proxy, and cancellation on core stop. All 211 regular Rust tests and all 58
real-Mihomo integration tests pass. Cargo check --workspace, Rust formatting,
warning-free all-target Clippy, TypeScript/Vite and changed-file Prettier pass.
The fresh target/mihomo-server-linux-x86_64-tls bundle builds successfully and its
checksums and bundled provenance/deployment documents match.

Actual-node smoke copies saved data into a temporary service and uses an unrelated
platform CA plus an empty SSL_CERT_DIR. The first of 56 traffic candidates returns
HTTPS 204 after node readback. Verified system-proxy subscription HTTPS then succeeds
via static roots and reaches YAML validation, correctly rejecting the empty 204
body without changing the catalog. Bypass is not enabled. Original data hashes
remain unchanged, and no node details, tokens or credentials are printed. The same
static-root/real-node smoke also passes with the release bundle binary.
All 21 Chromium workflows pass against the fresh release bundle, including strict
HTTPS rejection, retained failed drafts, explicit bypass import, saved flag/auxiliary
links across service restart, restoring strict validation and successful explicit
refresh after re-enabling the flag. This also re-verifies the previous MVP workflows.
HTTPS fixtures require OpenSSL in the development/test environment, not at runtime.
Fixture service/core and script-worker process counts are zero after cleanup.

Git handoff: no sandbox Git writes or commits; the external host script owns the
commit. The Linux MVP remains runnable and the full project is incomplete.
Next Delivery step 7 subtask: scheduled remote updates with bounded admission,
saved options, transactional refresh and shutdown cancellation. SOCKS/PAC, full
DNS/hosts/native TUN settings, resources, upgrades, backups/WebDAV, advanced pages,
immutable-file garbage collection and additional platform/deployment validation
remain pending.

## Previous increment: bounded scheduled subscription updates

Delivery step 7 now starts a subscription scheduler from the recovered catalog.
Remote rows with UID/URL, positive minute intervals and allow_auto_update other
than false are eligible. First run uses updated + interval, overdue rows run now,
missing/zero/future timestamps wait one interval. Success preserves upstream saved
options/linked enhancements and publishes the new timestamp through the existing
refresh transaction. Failure retries after a full interval without rewriting updated.
Restart recomputes overdue work; enormous unrepresentable delays remain dormant.

Profile watches reconfigure timers on metadata changes/deletion/manual refresh.
Running/retired UID state survives disable/re-enable until completion, preventing
duplicate automatic work. In-flight interval changes apply to the next full interval.
Queued work checks current source and policy before network access; actor admission
also checks policy, and full-option/file/URL guards reject stale downloads. Manual
refresh remains available when automatic updates are disabled.

At most four owned JoinSet workers share the existing four-download semaphore with
manual import/refresh. Due work beyond that limit stays in the schedule map instead
of spawning an unbounded queue. Network operations remain outside the actor. Active
refresh reuses settings/enhancement/validation/application/node restoration and
recoverable commit/rollback; managed downloads retain their saved route and defer
through transitional core phases. Direct/system updates can run with the core stopped.

A weak command sender preserves last-manager-drop cleanup. Shutdown cancels/drains
workers and waits for scheduler plus actor completion. Generic bounded scheduler
logs report starts/completions/failures without private input. Existing Web interval
and auto-update fields now control real schedules; helper text and the restart/
overdue workflow are updated.

All 223 regular Rust tests and all 59 real-Mihomo integration tests pass, including
five scheduling policy tests, seven loopback/paused-clock workflows and one active
managed automatic update/rollback workflow. Cargo check --workspace, Rust format,
warning-free all-target Clippy, TypeScript/Vite and changed-file Prettier pass.
The fresh target/mihomo-server-linux-x86_64-scheduler bundle builds and verifies
checksums plus bundled provenance/deployment document equality.

The release binary's isolated real-data smoke confirms the first of 56 traffic
candidates returns HTTPS 204 after node readback, with original data hashes unchanged.
Only the fixture copy disables existing automatic policies so real-provider URLs
are not fetched as a side effect of this proxy smoke; controlled loopback tests
exercise actual scheduled downloads. Verified static-root HTTPS fallback also
still reaches YAML validation through the system proxy with an unrelated platform
CA, rejects the empty 204 body and leaves the catalog intact. All 22 Chromium
workflows pass against the fresh release bundle. The new workflow persists an
enabled/overdue policy in the stopped fixture, verifies automatic refresh on restart,
checks refreshed raw content, disables automatic updates in the Web editor and
confirms manual refresh still works. Existing 21 MVP/TLS/proxy workflows also pass.
Fixture service/core and script-worker process counts are zero after cleanup.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable and full feature completion is still pending.
Next Delivery step 7 subtask: stable core upgrade resource metadata/download integrity,
followed by managed switching with rollback. SOCKS/PAC, full DNS/hosts/native TUN,
resources, Alpha upgrades, backups/WebDAV, advanced pages, immutable-file garbage
collection and additional platform/deployment validation remain pending.

## Previous increment: stable core release metadata and compressed preparation

Delivery step 7 now exposes authenticated core_release, prepare_core_upgrade and
prepared_core_upgrade commands. The official GitHub release API resolves latest
or a strict stable tag; exactly one published Linux x86_64 amd64-v2 asset must
match its fixed version URL, positive bounded size and SHA-256 digest. Missing
or conflicting metadata fails without downloading or accepting caller overrides.
Discovery is available before configuration; preparation/readback require the
existing bundle-managed persistent core. Query/preparation share one immediate
admission slot and run outside the lifecycle actor.

Direct HTTPS uses platform certificate verification, TLS 1.2/1.3, no implicit
proxy, no management credentials and a restricted five-redirect policy. Metadata
has a 20-second/1-MiB bound; package download has a 300-second/64-MiB bound.
Streaming checks declared length, SHA-256 and gzip magic before private atomic
publication. This establishes compressed-package integrity, not decompression,
executable validity, code signing or successful installation. The live core is
never replaced or executed by these commands.

Private .upgrade-staging directories and files use 0700/0600. Cancellation and
failed downloads remove temporary directories; startup cleans only owned pending
names without following links. Immutable version/digest IDs support cache reuse
and restart readback, which checks schema, target, URL, permissions, size and hash
again. Returned objects contain public release metadata and IDs, not local paths.
Cached candidates are retained; garbage collection remains pending.

All 231 regular Rust tests and all 60 real-Mihomo integration tests pass.
Seven new preparation tests and one authenticated command test exercise unique
metadata/assets, declared bounds, hash/magic/status failures, redirects/redaction,
private cache reuse/restart/tampering, symlink rejection, owned crash cleanup,
paused-clock timeout, cancellation, strict decoding and resource requirements.
The real-Mihomo gzip fixture independently decompresses for byte equality and
checks that the existing executable remains unchanged. Cargo check --workspace,
Rust formatting, warning-free all-target Clippy, TypeScript/Vite and changed-file
Prettier pass. The final target/mihomo-server-linux-x86_64-core-release-verified
bundle passes checksums and bundled provenance/deployment document comparisons.

A release-binary smoke against the actual official public API resolves v1.19.31,
downloads its 22,805,792-byte compressed amd64-v2 package, verifies SHA-256, reads
it back after service restart and confirms the existing core hash/stopped state
remain unchanged. This network check uses an isolated temporary bundle-managed
data directory and removes it afterward. A separate saved-node smoke uses an
isolated copy with automatic updates disabled only in that copy. The first of
56 traffic candidates returns HTTPS 204 after node readback; the existing
subscription static-root/system-proxy HTTPS path still reaches YAML validation
with an unrelated platform CA and rejects the empty body without catalog changes.
Original data file hashes remain unchanged; secrets are not printed.
All 22 Chromium workflows pass against the final release bundle. Fixture service,
core and script-worker process counts are zero after cleanup.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done.
Next Delivery step 7 subtask: bounded gzip decompression and staged executable/version/
configuration validation, followed by actor-owned core replacement, rollback and
interrupted-switch recovery. Core-download managed/system proxy routing, static-root
fallback, Alpha/other targets, upgrade UI, SOCKS/PAC, full DNS/hosts/native TUN,
resources, backups/WebDAV, advanced pages, garbage collection and additional
platform/deployment checks remain pending.

## Previous increment: bounded core extraction and actor-owned candidate validation

Delivery step 7 now adds stage_core_upgrade (prepared ID) and staged_core_upgrade
(stage ID). Admission covers query/download/staging/readback; the staging command
transfers its permit to the actor, retaining ownership if the HTTP caller leaves.
The actor captures its current normalized YAML and committed revision and serializes
validation with configuration/lifecycle changes. The existing core continues running;
this command neither replaces it nor changes runtime/profile/node records or status.

The package hash is checked again on the same compressed bytes used for extraction.
A blocking worker uses the Rust-backed flate2 decoder, with a 15-second cooperative
clock, shutdown checks, 64-KiB chunks and a 128-MiB uncompressed cap. CRC/truncation,
empty output, trailing bytes, concatenated members, scripts and non-x86_64 ELF
headers are rejected. Executables receive private 0700 permissions and fsync.
Version -v must report the exact published stable tag. Mihomo -t validates the
actor's YAML snapshot in a disposable data directory with private snapshots of
available known Geo files (256 MiB combined cap). Existing live Geo files are not
passed as its data directory. Missing/other provider resources may still fail
validation; the full resource pipeline remains pending.

Both executable probes have five-second deadlines and 64-KiB per-stream output
bounds, with cancellation, kill and reap. Failure messages exclude candidate
stdout/config diagnostics. After successful execution, executable/configuration
hashes are checked again. A private manifest is atomically published under an
immutable package-ID/config-hash stage ID; same-snapshot reuse still reruns probes
and verifies the old artifact. Its original revision proof is retained even if
identical YAML is subsequently committed under another revision. Readback checks
source-package integrity, manifest/schema/IDs, exact 0700 executable mode, ELF,
length/digest and the private configuration hash. Candidate records are historical
validation proofs; activation must recheck the then-current configuration/resources.
Shutdown/failure removes pending artifacts; startup's existing owned-pending cleanup
handles interrupted extraction. Completed candidate garbage collection is pending.

All 236 regular Rust tests and all 62 real-Mihomo integration tests pass. Four
new staging/probe unit tests cover CRC/truncation/extra members/trailing bytes,
size bounds, architecture/scripts, private cache/config/permission/link integrity,
version failure, bounded stdout/stderr, timeout, cancellation and process reaping.
A queued actor test aborts the caller and verifies admission remains held until
shutdown drains the actor and reaps its validator. Two new real-Mihomo workflows
exercise extraction/version/config validation, wrong-version/invalid-config rejection,
cache/restart/tampering, and preserved running PID/generation/config/core hash.
Existing management tests also verify authentication, strict staging schemas and
managed-resource requirements. Cargo check --workspace, Rust formatting,
warning-free all-target Clippy, TypeScript/Vite and changed-file Prettier pass.

The fresh target/mihomo-server-linux-x86_64-core-stage release bundle builds and
passes checksums plus bundled provenance/deployment document comparisons. An
isolated release-binary smoke downloads the actual official v1.19.31 package,
validates its compressed hash, extracts and probes the executable against the
bootstrap configuration, verifies candidate readback after restart and preserves
the installed core hash/stopped state. A separate isolated copy of saved data
validates a locally compressed real core against the actual generated subscription
configuration without changing PID/generation/runtime/executable. The first of
56 traffic candidates returns HTTPS 204 before and after candidate validation.
The subscription static-root/system-proxy HTTPS path still reaches YAML validation
under an unrelated platform CA and rejects the empty body without catalog changes.
Automatic policy is disabled only in the smoke's copied catalog; original data
hashes remain unchanged and no credentials or endpoints are printed.
All 22 Chromium workflows pass against the new release bundle. Fixture services,
cores, script workers and stage-probe Python processes are zero after cleanup.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done.
Next Delivery step 7 subtask: actor-owned managed-core replacement, health/readiness
verification, rollback and interrupted-switch recovery, using these verified staged
candidates and revalidating current configuration/resources before activation.
Core-download proxy routing/static roots, Alpha/other targets, upgrade UI, full
DNS/hosts/native TUN, resources, backups/WebDAV, advanced pages, garbage collection,
SOCKS/PAC and additional platform/deployment checks remain pending.

## Latest increment: managed core activation, health checks and durable rollback

Delivery step 7 now exposes activate_core_upgrade (staged ID) and core_installation.
The actor retains upgrade admission through replacement, including caller disconnect,
and serializes activation with lifecycle/configuration work. Current normalized YAML
must match the staged configuration hash; stale candidates fail before stopping the
core. Extraction/version/config probes rerun against current known Geo resources.
The copied activation executable is hash checked; no caller paths/checksums are accepted.

The private .core-upgrade journal stores bounded previous/candidate copies, hashes,
previous file mode/installation receipt and running intent. After stopping/reaping the
old child, one same-filesystem rename replaces the managed core. Startup checks the
private controller's version and actual proxy ports, then repeats health checks after
a short settle interval before the durable committed marker. Running cores restart
and restore saved node choices. Stopped cores temporarily start for live checks and
stop again before commit; their installation response records the verified version.
Runtime revisions, active profile, settings and selection records are preserved.
An explicit activation always performs replacement, including the same version;
the upstream force/no-op wrapper and Web interaction remain the next increment.

Pending failures stop/reap the candidate, restore the old bytes/file mode/receipt
and restart the previously running core unless service shutdown is underway. Retry
state from a failed candidate is cleared. A committed log retains the new core and
completes receipt/cleanup rather than rolling back after metadata failure. Startup
recovers before bundle seeding or any new child starts; actor admission and retries
also recover outstanding work. Interrupted preparation and rollback are idempotent.
Unexpected files/links, malformed metadata, missing/corrupt required backup, unknown
live bytes or committed hash/size conflicts retain recovery state and fail closed.
Copies/manifests use private directories/files, fixed names, size bounds and fsync.
The persistent receipt is checked against installed bytes on readback; initial bundle
seeding has no upgrade receipt. Candidate caches remain immutable and retained.

Linux ordinary core and validator children now set a parent-death SIGKILL with a
parent-PID race check, preventing orphan candidate execution across process-crash
recovery. Normal SIGTERM still uses existing graceful termination/reaping. This
workflow supports Linux x86_64 ordinary private executables; set-ID/file-capability
cores and native privilege/xattr transfer remain outside the migration. It verifies
bounded startup health, not indefinite stability after the commit point.

Verification for this increment:

- `cargo check --workspace --locked --offline` and workspace Clippy with
  `--all-targets -- -D warnings` pass; Rust formatting and existing Web formatting
  checks pass. The regular workspace run passes 242 tests; all 65 opt-in tests
  pass with the real `/usr/bin/verge-mihomo` and `--test-threads=1`.
- New storage tests cover pending/committed recovery, interrupted preparation
  and rollback, previous mode/receipt restoration, installed hash conflicts,
  corrupt backups, malformed metadata, unknown entries and symlink rejection.
  Actor tests verify running/stopped activation, new PID/inode, saved selection
  and configuration preservation, stale-candidate rejection before stopping,
  failed-candidate rollback and shutdown cancellation. A real service SIGKILL
  test verifies candidate termination/reaping and old-core recovery on restart.
- The final-source Linux release bundle is
  `target/mihomo-server-linux-x86_64-core-activation-verified`. All package
  checksums pass; bundled deployment/provenance documents match their sources.
  All 22 Playwright browser workflows pass against this final bundle.
- An isolated official-release API smoke verifies stable metadata, bounded
  download, SHA-256, extraction/version/configuration probes and activation of
  `v1.19.31` (22,805,792 compressed bytes). Its initially stopped state is restored
  after live health checks; prepared/staged records, installed bytes and receipt
  survive a service restart.
- An isolated copy of the actual saved subscription provides 56 traffic-capable
  nodes. The first tested node returns HTTPS 204 before and after activation;
  the candidate is staged without interrupting the old proxy, then activation
  replaces PID/inode and restores the selected node. Active profile/runtime
  readbacks and original `data` file hashes remain unchanged. The existing
  system-proxy/static-root subscription fallback also passes after activation.
  Automatic updates are disabled only in that disposable copy.
- After test shutdown, the fixture process audit finds zero services, cores,
  script workers or probe scripts left running. Isolated smoke directories are
  removed; the original data remains untouched.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done.
Next Delivery step 7 subtask: upstream-compatible stable upgrade force/no-op adapter
and Web upgrade workflow, including version/result readback and failure repair.
Core-download managed/system routing/static roots, Alpha/other targets, full native
TUN/DNS/hosts/resources, backups/WebDAV, advanced pages, garbage collection, SOCKS/PAC
and additional platform/deployment integrations remain pending.

## MVP completion boundary

The first usable MVP must provide an actual working path through the layers,
with the following observable behavior:

1. Start a persistent Rust service with explicit listening, authentication,
   data directory, resource directory, and Mihomo binary settings. Serve built
   Web assets from that service. Provide a reproducible build and launch command
   and at least one documented deployment path for the single service.
2. Keep the management API and page available before the core is ready, while
   it is stopped, and when configuration validation or core startup fails.
   Show current state and errors so the administrator can repair and retry.
3. Import a subscription, select it, edit configuration, generate a candidate
   configuration using the migrated processing flow, and validate YAML and
   `mihomo -t` before application. Preserve the working configuration when
   validation or application fails and make recovery behavior explicit.
4. Start, observe readiness, stop, and restart one managed Mihomo child. Prefer
   hot reload for valid configuration changes, preserving the upstream restart
   fallback. Serialize lifecycle operations and configuration application.
5. Query proxies and select a node through the backend. Persist configuration,
   the active subscription, and node selections; restore them after restarting
   the service. Show core logs and essential status in the browser, with
   cancellation and reconnection for realtime subscriptions.
6. Stop the service through its documented deployment path and verify that the
   child is terminated and reaped. Run the full import → validate → start →
   select → reload → service restart → restore workflow without a desktop
   environment.

MVP verification must exercise these real behaviors, including failed startup
and invalid configuration, rather than relying solely on compilation or mocks.
Keep the upstream tests and add targeted migration tests where needed.

## Delivery order

Continue in reviewable increments. Every increment must build and report its
remaining integration gaps. Prioritize the usable MVP before expanding features:

1. Extract independent crates, configuration models, and pure processing logic;
   retain source revisions, formats, and behavioral tests.
2. Extract the complete Tauri-free Mihomo client from the pinned plugin commit
   and verify the communication needed for the MVP. Retain its broader API.
3. Add the lifecycle manager, runtime state, and unified shutdown path.
4. Wire configuration and subscription persistence, enhancement, validation,
   application, and recovery into a working backend flow.
5. Add Axum, authentication, WebSocket adapters, and a small usable React UI
   for that flow. Serve its built assets through the Rust service.
6. Supply the initial deployment path and complete the MVP behavior checks.
7. Complete remaining remote subscription/enhancement workflows, then expand
   scheduled updates, stable/Alpha core upgrades with rollback, backups
   and WebDAV, media detection, advanced settings, the remaining UI pages,
   and additional platform release/service integrations from `headless.md`.

Deferred capabilities remain required by the full design. Preserve existing
business semantics during extraction, even when the initial UI exposes only
the operations needed for the MVP.

## Migration report requirements

Each migration report must state what changed, how it was verified, any
remaining integration limits, and the next step. Include the complete target
architecture with status markers, keeping scaffold status separate from
completed migrations. Update provenance for copied code and this document for
implementation status in the same increment.
