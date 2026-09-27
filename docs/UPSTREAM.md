# Upstream provenance

## Clash Verge Rev

- Repository: https://github.com/clash-verge-rev/clash-verge-rev
- Local checkout at initialization: `../clash-verge-rev`
- Pinned commit: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`
- Working tree at initialization: clean
- License: GPL-3.0-only
- Upstream toolchain: Rust `1.98.1`, with `rustfmt` and `clippy`
- Upstream crate edition: `2024`

The local checkout is a source reference, not a runtime or build dependency.
Extract from the pinned revision and retain license and copyright notices.
For each extraction, record the source path, destination, source commit, and
adaptations here.

| Source path | Destination | Source revision | Adaptations |
| --- | --- | --- | --- |
| `LICENSE` | `LICENSE` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; copied verbatim |
| `rust-toolchain.toml` | `rust-toolchain.toml` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; copied verbatim |
| `rustfmt.toml` | `rustfmt.toml` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; preserve upstream formatting without source churn |
| `crates/clash-verge-draft/src/lib.rs` | `crates/clash-verge-draft/src/lib.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; copied verbatim |
| `crates/clash-verge-draft/tests/test_me.rs` | `crates/clash-verge-draft/tests/test_me.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; reused existing behavior tests |
| `crates/clash-verge-draft/bench/benche_me.rs` | `crates/clash-verge-draft/bench/benche_me.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; retained existing benchmarks |
| `crates/clash-verge-draft/Cargo.toml` | `crates/clash-verge-draft/Cargo.toml` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | Inherit package metadata and dependencies from the new workspace |
| `crates/clash-verge-limiter/src/lib.rs` | `crates/clash-verge-limiter/src/lib.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; includes the two existing behavior tests |
| `crates/clash-verge-limiter/Cargo.toml` | `crates/clash-verge-limiter/Cargo.toml` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | Inherit package metadata; omit upstream workspace lint inheritance |
| `src-tauri/src/enhance/field.rs` | `crates/headless-core/src/enhance/field.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; pure field processing |
| `src-tauri/src/enhance/merge.rs` | `crates/headless-core/src/enhance/merge.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; includes the existing merge test |
| `src-tauri/src/enhance/seq.rs` | `crates/headless-core/src/enhance/seq.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | None; includes `SeqMap` and the three existing sequence tests |
| `src-tauri/src/enhance/mod.rs`: `use_sort_tests` | `crates/headless-core/src/enhance/field_tests.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | Extract existing test module; add imports for its new parent module |
| `src-tauri/src/config/prfitem.rs`: `PrfItem`, `PrfSelected`, `PrfExtra`, `PrfOption` | `crates/headless-core/src/config/prfitem.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | Extract data declarations unchanged; omit desktop, download, and item methods |
| `src-tauri/src/config/profiles.rs`: `IProfiles` | `crates/headless-core/src/config/profiles.rs` | `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` | Extract schema unchanged; use local model imports; persistence and selection restoration remain pending |

The workspace preserves upstream dependency requirements and features for
`anyhow`, `parking_lot`, `tokio`, `criterion`, `serde`, `serde_yaml_ng`, and
`smartstring`. Serde derive and smartstring serde features match their use by the
upstream backend. Its lockfile is seeded from the
pinned upstream lockfile and reduced to the dependencies needed by this
workspace. `headless-core` re-exports draft and limiter types without changing
their implementation. No Tauri or desktop dependencies are included in these
migrations.

The new `profile_schema` test protects the extracted serialization boundary:
subscription metadata, enhancement references, and persisted node selections
round-trip through upstream-format YAML, while transient uploaded content is
not persisted. Pure configuration tests are reused from upstream rather than
reimplementing the algorithms or inventing a new processing order.

## Mihomo plugin

- Repository: https://github.com/clash-verge-rev/tauri-plugin-mihomo
- Commit locked by the pinned Clash Verge Rev checkout:
  `ba8434c08c869916b8041d707ad66599bb5230b2`
- Evidence: Clash Verge Rev `Cargo.lock`, package `tauri-plugin-mihomo`

The exact locked commit was obtained and inspected before extraction. Its source
tree, `Cargo.toml`, `package.json`, and README contain no license declaration or
license file. The extracted `mihomo-client` crate retains the original author
metadata and deliberately does not inherit the workspace GPL declaration.
The plugin's licensing remains unresolved for publication or redistribution.

| Plugin source path | Destination | Adaptations |
| --- | --- | --- |
| `src/lib.rs`: builder, defaults, timeouts | `crates/mihomo-client/src/lib.rs` | Return an ordinary `Mihomo` client; omit plugin registration, app state, commands, and automatic debug watcher startup; add ordinary `WsMessage` payloads |
| `src/mihomo.rs`: requests and API methods | `crates/mihomo-client/src/mihomo.rs` | Preserve the API implementation; replace Tauri response bodies with `WsMessage` and the debug task spawn with Tokio; return the debug task handle; expose checked subscriptions for consumer cancellation |
| `src/mihomo.rs`: connection-count adapter | `crates/mihomo-client/src/mihomo.rs` | Accept `connections: null` as zero connections, required by real Mihomo v1.19.31 snapshots |
| `src/mihomo.rs`: payload correctness tests | `crates/mihomo-client/src/mihomo.rs` | Reuse eight correctness tests with the ordinary response type; omit two ignored desktop IPC serialization benchmarks; add one null-connection regression test |
| `src/models.rs` | `crates/mihomo-client/src/models.rs` | Retain Rust/serde models; remove TypeScript export annotations and `TS`; adapt disabled TUN null dns-hijack to an empty list without changing serialized types |
| `src/error.rs` | `crates/mihomo-client/src/error.rs` | None; error variants, serialization, and conversion macros retained |
| `src/stream.rs` | `crates/mihomo-client/src/stream.rs` | None; TCP, Unix socket, and Windows Named Pipe stream implementations retained |
| `tests/models_compat.rs` | `crates/mihomo-client/tests/models_compat.rs` | Reuse 22 compatibility tests; replace crate import/formatting; add one strict null-TUN regression |
| `Cargo.toml` | `crates/mihomo-client/Cargo.toml` | Remove Tauri, plugin build dependencies, build script, bindings generator, and desktop test setup; inherit common workspace dependencies and add features used by standalone tests |

All plugin entries above come from
`ba8434c08c869916b8041d707ad66599bb5230b2`. The workspace lockfile retains the
external dependency versions and checksums seeded from the pinned Clash Verge
Rev lockfile. No Tauri dependencies are used by the extracted client.

The new `live_core` integration tests exercise the standalone transport and
callback adaptation with an explicitly supplied real Mihomo binary, rather than
using the upstream tests' fixed desktop controller, credentials, and `.env`.
They own their temporary configuration and child process. Windows transport and
remote fetch/update APIs still require platform-specific and network validation.

## Standalone lifecycle adaptation

`service/src/core_manager.rs`, `service/src/shutdown.rs`, and the new entry point
are service-specific implementations, informed by the pinned Clash Verge Rev
sources below. They are not verbatim extractions of the desktop manager.

- `src-tauri/src/core/manager/state.rs`: `-d`, `-f`, platform controller arguments,
  immediate pipe draining, version-based readiness, and owned child cleanup.
  The standalone implementation retains 30 probes, 400 ms probe timeouts, and
  100 ms probe intervals.
- `src-tauri/src/core/manager/lifecycle.rs`: serialized lifecycle transitions and
  separating readiness from successful spawn. Desktop proxy controls, external
  service sessions, Service/Sidecar handoffs, and staging are omitted.
- `crates/clash-verge-signal/src/unix.rs`: SIGTERM/SIGINT feed a unified shutdown
  flow. The standalone implementation uses the existing Tokio runtime rather
  than importing the desktop logging dependency and creating another runtime.

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.

The service adds an exclusive data-directory lock, an actor owning the child and
serializing operations, status/log channels, bounded recovery, cancellation of
startup/reload on shutdown, and explicit graceful-stop/kill/reap timeouts. Its
Unix signal-send implementation addresses only the actor's owned unreaped child.
Clap and libc use versions already present in the pinned upstream lockfile.

New lifecycle tests target the runtime adaptation risks: directory ownership,
duplicate and concurrent operations, failed startup with subsequent repair,
accepted/rejected reloads, restart, bounded crash recovery, shutdown during
readiness, forced termination, and actual foreground service signal handling.
The tests use isolated owned cores and do not operate on desktop instances.

## Runtime configuration adaptation

`crates/headless-core/src/config/runtime.rs` and `service/src/validation.rs`
implement service-owned runtime storage and ordinary process validation.
The configuration methods in `service/src/core_manager.rs` wire them into the
same lifecycle actor. This is new adaptation code, not a verbatim copy of the
desktop config singleton or validator.

Sources inspected at `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`:

- `src-tauri/src/core/validate.rs`: retain `-t -d <data> -f <candidate>`, process
  exit checking, and the stderr fatal keywords. Replace Tauri sidecar execution
  with an owned Tokio child. Add a configurable five-second core-test deadline,
  shutdown cancellation, kill/reap, and bounded concurrent stdout/stderr capture.
  Do not copy the desktop validation cache, singleton, script paths, or claim
  the structured outcome types have been migrated.
- `src-tauri/src/core/manager/config.rs`: retain validate → apply → commit ordering
  and hot-reload → restart fallback. Replace desktop/service staging and global
  draft access with immutable candidates, a commit journal, and explicit recovery
  of the prior configuration on failure. Runtime revision state is a new headless
  format; subscription files retain their separate upstream schema.
- `src-tauri/src/enhance/merge.rs`: the already extracted `use_merge` implementation
  is called directly for overlays; DNS shallow merge, hosts replacement, and
  ordinary deep-merge behavior are preserved. The full enhancement entry point,
  scripts, profile sequence, and user DNS/TUN settings are still pending.

The actor holds directory ownership during every write and configuration operation.
Runtime revision and state files are created with mode 0600 on Unix. State updates
write and sync a temporary file, rename it in the same directory, and sync the
directory. Startup never promotes an unfinished candidate. Disk and core rollback
errors remain observable; provider/Geo side effects and total resource transactions
are not covered by the runtime YAML journal. Tests cover interrupted applications,
manifest failures, invalid candidates, real service/core restoration, deterministic
reload/restart failures, validator timeout/cancellation, and output bounds.
## Local subscription persistence adaptation

`crates/headless-core/src/config/profile_store.rs` implements service-owned local
import and catalog persistence on top of the previously extracted upstream models.
The lifecycle actor now serializes profile import/selection alongside configuration
and process commands. The implementation is an adaptation, not a verbatim extraction
of the complete desktop profile feature.

Sources inspected at `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`:

- `src-tauri/src/config/profiles.rs`: retain `profiles.yaml` current/items and
  `profiles/<file>` layout, current item lookup semantics, the single-filename
  restriction, and known metadata during writes. Replace global `Config`, tray,
  Tauri handles, and write locks with the sole actor's owned store. Malformed
  metadata is reported rather than silently replaced with an empty catalog.
- `src-tauri/src/config/prfitem.rs`: retain local UID prefix `L`, type `local`,
  `<UID>.yaml`, name/description and epoch-seconds updated metadata. Generated
  IDs use the service's timestamp/process/counter mechanism. Preserve the
  original raw YAML and existing optional fields. This increment does not port
  automatic merge/script/rules/proxies/groups item creation from `from_local`;
  those constructors/templates and their enhancement execution remain pending.
- The already extracted `IProfiles` / `PrfItem` / `PrfOption` serde models are
  used directly; no desktop file format is replaced with a headless-only catalog.

Local import only checks YAML mapping syntax; activation separately executes the
runtime validation/application transaction. This separation permits retaining an
invalid imported item for later repair without changing the active runtime. Remote
downloads, updates, editing/deletion, and linked enhancement execution are pending;
linked profiles are rejected explicitly rather than applied without their links.
At that increment, node records survived metadata writes; runtime restoration
was implemented in the subsequent adaptation below.

The runtime manifest adds defaulted active/pending profile IDs. The active ID and
runtime revision share one commit point. `profiles.yaml.current` is a compatibility
mirror saved before that commit and repaired from the journal after interruption.
Standalone overlays retain profile identity, while full runtime imports clear it.
Profile writes use the existing synced temporary-file/rename helpers and Unix 0600
permissions. Tests cover schema/content preservation, cached remote metadata,
filename/symlink rejection, failed catalog saves, core validation and restart
failures, selection rollback, interrupted mirror recovery, actual CLI import/select,
and service restart without the original source.
## Node selection reconciliation and actor restoration

`service/src/selections.rs` extracts the pure selected-node planning functions
from `src-tauri/src/config/profiles.rs` at
`b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`:

- `SelectedNodesPlan`, `node_is_available`, `selected_nodes_need_confirmation`,
  `reconcile_selected_nodes`, `remaining_activations`, and `unsettled_selections`.
- Sixteen relevant upstream pure tests, including group membership, duplicate
  records, stale-record confirmation, and unloaded-group behavior. Only imports,
  item visibility, and test-module wrapping are adapted. Desktop/global-generation
  and unrelated record-removal tests are not copied; actor supersession and unfix
  are verified through targeted runtime tests instead.

The service directly reuses these functions and client model types. Add the
existing workspace `smartstring` dependency to preserve upstream selected-record
types. No new external dependency versions are introduced.

Profile-store save/record/forget methods retain `PrfSelected { name, now }`,
per-profile scope, group replacement, and removal of the serialized field when
the last entry is forgotten. Recording normalizes duplicates for the updated
group. The actor adapts the upstream restore worker and KeepRecords/Prune modes:
retain records on startup, reconcile stale records after application, and cancel
older restoration when a manual operation or lifecycle change supersedes it.
Replace global generation counters/background mutators and tray/refresh calls
with one owned restoration state, serialized command/timer turns, and ordinary
status/profile watches. Retain default first-pass, operation, settle, and retry
durations (3 s / 10 s / 30 s / 1 s). Both startup and apply restoration retry
incomplete groups; a previously accepted choice can be retried if a later snapshot
shows the core moved away. Queries/selection calls are cancelled during shutdown.

Manual node operations add explicit group membership checks, runtime confirmation,
and metadata/runtime rollback attempts. They require an active profile instead of
silently reporting an unpersisted choice for a standalone runtime. Automatic unfix
uses the retained `unfixed_proxy` API and forgets the saved record. Failures of
selection, persistence, rollback, or settling remain observable without declaring
the core stopped merely because a saved node is unavailable.

Real tests disable Mihomo's selection cache and verify node/service restart
restoration, profile isolation, automatic URLTest pin/unfix, invalid selections,
and failed disk persistence. Controlled tests verify delayed/unloaded groups,
manual supersession, refused/unconfirmed PUT responses, restoration deadlines,
and shutdown during a blocked query. Additional Fallback/LoadBalance-specific
live tests and Windows validation are still pending. The minimal example routes
through its Main selector and disables the core selection cache.

## Authenticated HTTP service adaptation

`service/src/management/{mod,auth,http}.rs` and entry-point HTTP lifecycle wiring
are new service code implementing the boundary in `headless.md`; no desktop
HTTP server or Tauri command registry is copied. Axum is pinned to 0.8.9, matching
the upstream checkout's lockfile version. Registry sources/checksums and the
new dependency set are recorded in this workspace's `Cargo.lock`.
Authentication uses the already selected `getrandom` and `url` dependencies;
Reqwest is a test dependency; Tower is also used by static serving below.

The allowlist calls existing manager/client APIs, preserving runtime validation,
reload/restart fallback, rollback, active profile commits, and node persistence.
Profile upload and committed-config reads add actor messages rather than opening
another store or controller. Authentication, exact Host/Origin policy, private
token publication, HTTP envelope/error adaptation and shutdown admission are new
service responsibilities. They do not change upstream profile or runtime formats.

Verification includes five targeted regular authentication/HTTP tests and one
real service/core HTTP workflow, plus rerun lifecycle/node integration tests.
WebSocket/browser adaptation and broader rules/providers/connections commands
remain pending at that increment; the HTTP slice does not establish the complete MVP.

## Browser WebSocket service adaptation

`service/src/management/websocket.rs` is new transport code. It reuses manager
status/profile watches and core output, plus the pinned client's checked traffic,
memory, connection/count and log subscription methods. No parallel Mihomo request
or frame-processing implementation is introduced. Payload models and the client's
JSON/error/count adapters remain unchanged. Browser authentication is adapted to
a first message because native browser WebSocket cannot set custom request headers;
the upgrade still checks the HTTP Host/Origin policy and never accepts URL tokens.

`mihomo-client::Mihomo::cancel_ws_connection(id)` adds synchronous targeted
cancellation using the existing reader-cancellation registry and connection map.
Service subscription guards call it on drop; they never clear other consumers'
subscriptions. Existing disconnect/global lifecycle cancellation APIs are retained.
Cancellation guards, bounded channels/samples, service retry/heartbeat/admission,
snapshot/reset events and upgraded-task draining are new service responsibilities.

Enable Axum 0.8.9's `ws` feature. Axum uses tokio-tungstenite/tungstenite 0.29.0;
the pinned upstream client retains its 0.28 implementation. These are separate
transport boundaries, and both dependency sets are recorded in Cargo.lock.
Tests use the 0.29 browser-side client and preserve the existing 0.28 real-core
tests. Validation exercises all five real feeds, multi-consumer cancellation,
core restart/retry, event snapshots and actual CLI shutdown with open sockets.
At that increment, React adapters/assets were pending; the adaptation below now
connects them. Broader domain events and Windows runtime checks remain pending.

## Minimal React UI and static serving adaptation

`web/src` contains new minimal browser views and transport adapters for the
headless management boundary. This increment does not copy or claim migration
of the complete upstream React/MUI desktop pages. React/react-dom 19.3.0,
TypeScript 6.0.3 and Vite 8.3.0 follow the inspected upstream frontend versions;
exact frontend dependencies are recorded in `web/package-lock.json`. Playwright
1.58.2 is a test-only dependency. No Tauri JS API or direct browser-to-core
controller connection is used. Existing backend models and pure upstream
configuration/selection logic remain the business implementation.

`service/src/management/assets.rs` adds canonical-directory static serving with
Tower HTTP 0.6.11's filesystem service and percent-encoding 2.3.2. Both versions
and their transitive registry checksums are locked in Cargo.lock. Tower's existing
0.5.3 util dependency is now a runtime dependency. Static/login requests keep
Host/Origin checks while authenticated API commands and first-frame WebSocket
contracts stay separate. The SPA fallback is limited to explicit navigation paths;
API paths, traversal and external symlinks are rejected.

The actor adds `edit_config` for complete runtime replacement that retains the
active profile ID. It shares existing validation/apply/journal/mirror/rollback
rather than introducing another configuration store. Existing standalone full
apply/import still clears profile identity; overlay retains it. Runtime edits do
not alter saved raw profile files or extracted upstream profile formats.

Real Mihomo v1.19.31 regression returned a null `tun.dns-hijack` list from `/configs`.
The extracted `TunConfig` deserializer now maps null to an empty Vec, retains
nonempty string lists and rejects non-string elements. Existing serialization
remains unchanged. This narrow adaptation is covered by a new model compatibility
test and both real HTTP/Unix client tests.

Three static-serving tests cover the new boundary. A real Chromium workflow
uses the production build, actual Rust listener and Mihomo child, verifies repair,
configuration/profile/node restoration and realtime data, checks mobile layout,
and requires successful SIGTERM cleanup. These checks establish the minimal
browser slice; full desktop feature parity and deployment remain pending.

## Initial Linux bundle and persistent core adaptation

`service/src/resources.rs`, `scripts/package_bundle.py`, `deploy/launch.sh` and
`deploy/mihomo-server.service` are new headless adaptations. The inspected
`../clash-verge-rev/scripts/prebuild.mjs` at
`b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b` maps Rust targets to independent
stable/Alpha Mihomo resources, prepares separate binaries and computes SHA-256.
The headless boundary retains that independent-binary/resource model and replaces
Tauri sidecars with one Rust-owned child and a writable persistent core directory.

The initial packager supports only `x86_64-unknown-linux-gnu`, using explicit local
core/version/hash inputs and ELF checks. It does not copy upstream latest-release
resolution or network download code, and does not yet implement the full platform
map, Alpha, Geo/provider resources or automatic fetching. Input and copied-core
checksums are verified; the resource manifest records the exact target and pin.
The release build uses ordinary locked npm/Cargo steps, with no build-time core
download or embedded binary extraction. Licensing metadata remains unchanged;
local bundle preparation does not resolve the pinned plugin's publication status.

Ring 0.17.14 was already present in Cargo.lock and is now a direct service dependency
for streamed SHA-256 verification. The small build script records Cargo's TARGET
for runtime manifest checks; it performs no I/O or downloads beyond that build
metadata. The manager initializes the core only after taking the existing data
lock, using synced private staging and non-overwriting publication. Existing core
files remain authoritative across service updates; independent core upgrade,
rollback and migration logic is still pending.

New regular resource tests verify initialization, checksum failure cleanup,
manifest/path/permission rejection and preserved upgraded cores. Python tests
protect packager boundaries. A real isolated bundle test verifies deployed repair,
state/node restoration, relocatable paths and child reaping; the existing Chromium
workflow also passes through the release launcher. The user systemd template is
statically verified with its ExecStart rendered to the built launcher, but systemd
runtime installation/execution remains pending.

## Direct remote subscription import adaptation

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.
`crates/headless-core/src/config/remote.rs` extracts/adapts pure portions of
`src-tauri/src/config/prfitem.rs::from_url`: dirty URL repair, BOM removal,
proxies/provider key checks, storage-prefixed subscription-userinfo, filename*,
filename, home URL normalization, update interval precedence/conversion and
auto-update default. `normalize_profile_home_url` uses url::Url rather than Tauri;
`src-tauri/src/utils/help.rs::parse_str` is reused unchanged. Metadata/body processing
accepts ordinary header pairs, not an HTTP framework or desktop singleton.
Safe service limits bound URL/name/body sizes, use checked interval multiplication,
require UTF-8 and provide a nonempty fallback name. These narrow differences are
covered by dedicated tests; this is not a full extraction of PrfItem::from_url.

`service/src/remote.rs` adapts the inspected `utils/network.rs` direct transport:
no proxy, ten redirects, existing reqwest TLS defaults, upstream-style user agent,
keepalive/no idle pooling and the actual from_url default of 20 seconds. Reqwest
0.13.5 was already locked and used by the client/tests; it is now a regular service
dependency. URL/percent-encoding versions already locked are also used by the
HTTP-independent core helper. TLS static-root fallback and invalid-certificate
bypass remain pending. Managed/system proxy modes and auxiliary defaults are
implemented in the later increments documented below; unsupported options remain
rejected.
The service adds timeout/concurrency/body limits, strict HTTP(S) validation and
transport error URL removal. Live third-party HTTPS/provider checks remain pending.

Remote imports persist upstream R UID/type/url/extra/home/option/updated fields and
raw YAML using the owned profile store's existing synced private writes. Local and
remote imports share the catalog commit/cleanup path. The store does not activate
an imported item, overwrite existing items, invent enhancement references or fetch
again on startup. Existing full activation/node restoration behavior remains intact.

Network futures run outside the lifecycle actor under four shared permits; only
completed downloaded content is sent for a serialized store commit. Shutdown or
consumer cancellation drops network futures, and abandoned queued commits are
skipped. HTTP exposes import_remote_profile with a strict supported-options shape;
the browser adds a URL form and remote usage rendering. Manual refresh is connected
by the following increment; edit/delete and scheduled updates remain later work.

Verification adds three pure tests, two store tests, one regular input/options
test and four opt-in HTTP/socket/real-core tests. The real Chromium workflow uses
an owned HTTP provider fixture for remote success/failure and restart persistence;
no user's credentials or real subscription service are used.

## Manual remote refresh and coordinated recovery adaptation

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.
`PrfOption::merge` is copied from `src-tauri/src/config/prfitem.rs` with public
visibility, retaining all supplied-field precedence and absent-option branches.
`src-tauri/src/config/profiles.rs::update_item` and
`src-tauri/src/feat/profile.rs::update_profile` are the behavioral references:
keep UID/title/description/URL/selected records, replace extra/updated/home, merge
options, permit manual refresh with auto-update disabled and apply active updates.
No desktop scheduler, singleton, tray refresh, emit or notification call is copied.

The service deliberately strengthens failure persistence: upstream writes new raw
subscription content before checking active core application and can report that
it saved the subscription without confirming application. Here an active invalid
or failed update attempts to preserve the previous saved subscription and runtime.
`headless-core/config/profile_store/refresh.rs` implements new immutable candidate
file pointers and a private journal tied to the runtime revision. Recovery follows
the runtime commit for active updates and the catalog rename for inactive ones;
old files are retained and garbage collection remains pending. The upstream `file`
field remains a single filename; a refreshed item can point to refresh-*.yaml
instead of the original UID.yaml. The UID itself remains unchanged.

The existing lifecycle actor owns validation, reload/restart fallback, catalog
publication, runtime commit and rollback. It checks file/URL/options against the
network snapshot but reads node records at commit time. Shared download admission,
shutdown cancellation and unsupported proxy/TLS/enhancement rejection also apply
to refresh. Manual refresh replaces runtime-only edits with downloaded profile
content; existing selection reconciliation applies after a successful update.
HTTP and browser adapters expose refresh_profile without options overrides.
Scheduled updates and broader profile editing/deletion remain pending.

Five new pure/store tests verify upstream option precedence, preserved identity
and node records, interrupted journal outcomes, write failures and unsafe paths.
Three added socket tests use owned providers and real/controlled cores to check
noncurrent/active/stopped updates, validation/application failure recovery, node
choices during download, stale completion and shutdown. Authenticated HTTP and
release-bundle Chromium tests verify UI/API refresh and restart persistence.

## Restricted profile metadata editing and recoverable deletion

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.
`src-tauri/src/config/profiles.rs::patch_item` and `plan_delete_item` are behavioral
references, not verbatim copies. The service store adds edit.rs and delete.rs;
all disk mutations still run under the directory lock through the lifecycle actor.
Editing retains upstream supplied-field patch semantics for name/desc/URL, but
uses a strict service allowlist: UID/type/file/selected/usage/timestamp/enhancement
references cannot be set by HTTP. Supported download option fields are merged
using the previously extracted PrfOption::merge instead of replacing the whole
option object. URL and byte/timeout/header limits are validated before catalog save.
Only local and remote profiles are supported. Null/omitted patch fields retain
saved values; an empty description clears the visible description. Changing URL
or options does not fetch, apply YAML, update the download timestamp or reset
cached provider metadata; those update after an explicit successful refresh.

Deletion deliberately protects the canonical active UID (even while stopped) and
the compatibility current mirror. Upstream can automatically choose another current
profile and cascade auxiliary deletion. That behavior is deferred until linked
enhancement ownership and generation are implemented; linked or referenced items
are explicitly rejected here. Shared raw files in copied catalogs are retained
while another item references them. The service never accepts browser file paths.

A new private profile-delete.yaml journal records only the UID and safe raw-file
name. Catalog removal is the commit point and precedes raw-file removal. Startup
or the next actor command clears an uncommitted plan or completes committed file
cleanup; failures retain a recoverable journal and report their outcome. Missing
raw files are tolerated, external symlinks/directories are rejected, and catalog
save failure does not remove content. Only the current raw pointer is cleaned;
older unreferenced refresh/runtime revisions remain pending garbage collection.
Deletion during download cannot recreate the removed UID. Name/description edits
survive an in-flight refresh; URL/option edits make its snapshot stale.

Six added store tests cover field preservation, input/disk failures, current UID
protection, interrupted deletion, cleanup retry, shared files, enhancement links
and unsafe paths. An authenticated HTTP test verifies strict ownership/option
fields and actor mutations. An owned-provider/real-core integration verifies
current/stopped protection, no runtime or network side effect from editing,
concurrent refresh behavior, content cleanup and service restart. The browser
workflow exercises editing, retained credentials/metadata, deletion confirmation,
current protection, both local/remote removal and persistence through restart.

## Linked YAML merge generation and coordinated catalog/runtime adaptation

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.
`src-tauri/src/config/prfitem.rs::from_merge` provides the schema reference:
lowercase `m` UID, matching `.yaml` filename, type `merge` and updated timestamp.
`src-tauri/src/enhance/mod.rs::process_profile_items` provides the per-profile merge
reference, applying `use_merge` before script execution. The already copied
`src-tauri/src/enhance/merge.rs` is reused unchanged for runtime generation,
including lowercase overlay keys, recursive ordinary merge, shallow DNS fields,
hosts replacement and array replacement. No new desktop source is copied here.

`headless-core/config/profile_store/merge.rs`, the lifecycle actor and browser/API
commands are service adaptations. An edit creates immutable content and a fresh
auxiliary row linked by `PrfOption.merge`, preserving raw base content/identity and
node metadata. A private journal stores previous/candidate catalogs together to
coordinate auxiliary ownership and link publication with runtime acceptance. Active
edits validate/apply before accepting catalog/runtime changes; inactive edits commit
at catalog publication and receive core validation on selection. Recovery follows
runtime acceptance for active changes, catalog acceptance for inactive changes.
This strengthens desktop save-before-core-confirmation behavior without changing
the extracted pure merge semantics. Remote refresh retains the link and applies
saved merge content at actor commit. Shared/reserved global `Merge` rows survive
detach; unreferenced ordinary auxiliary rows are retired while immutable files
remain pending garbage collection. Base deletion still requires explicit detach
and a separate profile switch; automatic cascade/current switching is not copied.

The full upstream global Merge/Script ordering, linked sequence/script execution,
authoritative application settings restoration and DNS/TUN/settings pipeline are
not implemented by this slice. The service instead guards private-controller
fields after generation and keeps existing raw/runtime settings behavior.
Imports do not automatically create auxiliary/default global items. Unknown/nested
links are rejected rather than silently skipped; the browser displays only base
profiles, while catalog queries retain upstream auxiliary rows.

Five store tests verify raw/node retention, upstream merge semantics, shared links,
input/write failures, interrupted commit outcomes and unsafe journal/content. An
authenticated HTTP test checks strict read/set/clear commands and private-controller
restrictions. Two real/controlled-core tests check selection, refresh, invalid
rules, catalog/reload/restart failures and restart restoration. A fourth Chromium
workflow verifies raw preservation, auxiliary filtering, editing, rejection, service
restart and detach against the fresh release bundle. The client integration fixture
also waits for proxy/rule initialization rather than controller availability alone,
which exposed a startup race during full-suite verification.

## Linked sequence generation and shared enhancement recovery adaptation

Source revision: `b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`.
Schema references are `src-tauri/src/config/prfitem.rs::from_rules`, `from_proxies`
and `from_groups`: lowercase r/p/g UID, type rules/proxies/groups, matching YAML
filename and timestamp, linked by PrfOption. `src-tauri/src/enhance/mod.rs::enhance`
and `process_seq_items` establish rules → proxies → groups before the per-profile
merge. The previously extracted `src-tauri/src/enhance/seq.rs` remains unchanged:
prepend/keep-not-deleted/append, proxy-reference removal and insertion of added
names into the first existing selector. Group edits occur after that insertion.
The subsequent extracted merge can replace arrays produced by sequences.

`config/profile_store/sequence.rs` is a new strict service input adapter. It
requires the same three SeqMap fields, rejects unknown/missing/null fields and
checks rule strings and proxy/group name/type mappings before handing the values
to unchanged upstream processing. Invalid Mihomo-specific content is rejected on
active application/selection, not silently ignored. The service does not copy
desktop automatic auxiliary item creation, default fallback links, scripts,
authoritative application settings or final group cleanup in this increment.

The merge-only service transaction is generalized for all four auxiliary kinds;
EnhancementContent/EnhancementPlan and begin/publish/recover_enhancement are the
shared interfaces, with original merge store entry points retained. Its journal
keeps profile-merge.yaml/schema version 1 so existing pending merge edits recover
after upgrading. A strict kind field defaults to merge for older journals. Fresh
auxiliary files and previous/candidate catalog snapshots coordinate storage with
runtime acceptance; active edits use -t/reload/restart/rollback and inactive edits
commit at catalog publication. Shared and reserved Merge/Rules/Proxies/Groups rows
remain on detach; retired ordinary rows leave immutable content for future GC.
Remote refresh applies current saved sequences/merge and rejects source snapshots
superseded by link changes. Node records remain attached to the original base UID.

Four store tests cover ordering/merge precedence, preservation, strict fields,
shared links, interrupted active/inactive outcomes, legacy merge journals, catalog
failure and unsafe content. One HTTP test covers authenticated strict read/set/clear
commands. Two real/controlled-core tests cover invalid application, catalog/restart
failure rollback, selection/refresh, stale downloads and stopped/restart behavior.
A fifth release-bundle Chromium workflow covers all three editor types, raw
preservation, rejection, restart/node restoration, mobile layout and detach.

## Bounded linked script processor and service worker adaptation

Source revision: b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. References are
src-tauri/src/enhance/script.rs::use_script/eval_script,
src-tauri/src/enhance/mod.rs::process_profile_items and
src-tauri/src/config/prfitem.rs::from_script. The new enhance/script.rs adapts
the processor rather than copying its desktop async/logging/template dependencies.
Boa is pinned to the same 0.22.0 version and default features; new dependency
versions/checksums are recorded in Cargo.lock. Evaluation uses the existing copied
lowercase helpers, main(config, name), JSON config/result conversion, six console
methods/formatting and upstream 1000-entry/1-MiB console, 10-MiB JSON and
10-million-loop limits. The [Boa 0.22 Context documentation](https://docs.rs/boa_engine/0.22.0/boa_engine/context/struct.Context.html)
was checked for the host property/callable/evaluation boundary.

Service adaptations bind both config JSON and profile name as properties instead
of interpolating escaped names, require a synchronous object return, enforce
8-MiB YAML/private-controller limits and reject failed transactions instead of
upstream's unchanged-config fallback. Captured diagnostics remain observable.
The upstream identity-template shortcut/changed-key tracking and full global/
authoritative/final-cleanup processing are not migrated by this slice; the template
can execute normally. The native logger captures Rust-owned data only, preserving
the Boa closure's GC safety requirement. No filesystem/network/Node host API is
registered, and promise jobs are not executed.

Upstream wraps spawn_blocking with a five-second timeout, which cannot stop an
already running thread. The service instead runs evaluation in a disposable
same-binary worker before constructing a Tokio runtime. On Linux, worker address
space is limited to 512 MiB, CPU to five seconds and core dumps disabled; parent
wall time is at most five seconds. IPC/stderr sizes are bounded, environment is
cleared, and timeout/shutdown kills and reaps the worker. This adds process/resource
isolation, not an OS sandbox. Other platforms explicitly reject execution until
equivalent limits are implemented. Script source is limited to one MiB.

Script persistence reuses the generalized enhancement plan/journal with kind script,
immutable s*.js rows and PrfOption.script, retaining shared/reserved Script rows.
Inactive saves execute without core validation; selection/active changes/refresh
run sequences → merge → script, then validate/apply/commit with existing rollback
and node recovery. Startup restores committed runtime without re-executing scripts.
Tests cover safe name/casing, failure logs/types/controller limits, raw/node/source/
shared-link preservation, interrupted edits, strict authenticated commands,
real/controlled-core failures and restoration, worker time/memory/output limits,
shutdown/reaping and the sixth release-bundle Chromium editor workflow.

## Global defaults and staged profile generation adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src-tauri/src/config/config.rs::ensure_default_profile_items`,
`src-tauri/src/utils/tmpl.rs::ITEM_MERGE/ITEM_SCRIPT`, and
`src-tauri/src/enhance/mod.rs::collect_profile_items/process_seq_items/
process_global_items/process_profile_items/enhance`.

`config/profile_store/defaults.rs` copies the two template strings and adapts
initialization to the service-owned store. Reserved Merge/script rows retain UID
Merge/Script, types merge/script, YAML/JS files and epoch-second timestamps.
Unique immutable filenames prevent overwriting files from interrupted creation;
both missing rows publish in one catalog rename after files are synced. Existing
rows/source/catalog metadata are preserved; existing catalogs without a loaded
items list are left alone, matching upstream. Initialization runs after refresh,
enhancement and deletion recovery, with explicit pending-journal rejection.
Unpublished files are removed on ordinary failure; interruption can leave unused
files for future garbage collection. Automatic per-profile auxiliary construction
is still pending and explicit editor reads continue to expose saved links only.

Generation now exposes stages rather than silently premerging the profile before
the global script: rules/proxies/groups sequences, global Merge, global Script,
profile merge, profile script. Both scripts receive the base profile name and use
the existing bounded service worker. Exact untouched identity template strings
can skip worker creation. Upstream missing-link fallback to Merge/Script and
optional reserved Rules/Proxies/Groups rows is preserved. Consequently, an
unlinked profile applies global Merge/Script again in its profile stages; a
global script with side effects can run twice. This is upstream behavior, not
deduplication. Absent reserved sequence rows have empty sequence defaults.
Explicit broken links, unsafe files, wrong types, nested options and malformed
sources fail explicitly instead of upstream's lenient fallback.

The pure mapping convenience API rejects nonidentity scripts, preventing callers
from accidentally bypassing a required worker. Selection, active refresh and
linked enhancement edits all execute staged generation before existing runtime
validation/commit/recovery; failures preserve committed state. Startup still
restores the committed runtime without running global or profile scripts.
Global Merge's template sets profile.store-selected true, as upstream does; the
actor continues restoring its persisted per-profile node choices.

This increment does not expose global editing commands or a browser editor:
existing imported global rows are consumed, and new defaults are persisted.
Authoritative service settings, builtins/DNS/TUN and final group cleanup/sort are
still pending. Global editing must next use validated catalog/runtime transactions
rather than rewriting live source files. Tests cover default preservation,
publication failure/journal ordering, reserved fallback and invalid content,
stage order, real-core refresh/detach/node restoration, and failed global
regeneration after a committed-runtime restart.


## Transactional global Merge/Script editing adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src-tauri/src/cmd/save_profile.rs::save_profile_file/profile_affects_runtime/
handle_saved_profile_file/restore_original` and the previously documented global
collection/order/templates. Upstream writes the old filename first, validates,
restores its bytes on rejection and applies affected runtime. This service uses
new immutable files and publishes a reserved UID's catalog pointer only through
validated runtime/catalog commit. It does not copy Tauri globals, validation
notice events, forced-update singleton or automatic backup triggers; automatic
backups remain pending. Globals always regenerate an active profile, including
profiles with explicit links, because the connected pipeline always runs globals.

The existing enhancement journal is extended with default-false global scope.
Linked transactions continue using option-link markers; global transactions use
Merge/Script file-pointer markers, with unchanged schema version and old-record
recovery. Both scopes retain runtime-revision acceptance, catalog-only acceptance
without an active profile, precommand/startup recovery and sole actor ownership.
Global journal checks additionally prevent unrelated catalog mutations, permit
first-row creation and reject unsafe/wrong-type/nested/nonreserved targets. Base
metadata/links/raw/nodes and other global sources are retained. Previous immutable
files await garbage collection, allowing rollback without rewriting bytes.

New strict authenticated commands read/set/reset each global stage. Reset restores
the upstream template under the same reserved UID, with a new file pointer.
Active changes use staged generation, workers, Mihomo validation, reload/restart
rollback and node restoration; stopped profiles stay stopped. Proposed global
content is resolved for both the global stage and explicit/default reserved
profile links. Replacement can repair broken old source without evaluating it.
Without an active UID, neither standalone runtime nor child PID changes. Global
merge validation uses overlay normalization before controller checks, including
uppercase fields; global scripts parse the exact execution program in the existing
bounded worker without running top-level code or main. Presence/behavior of main,
config-dependent errors and Mihomo validity defer to selection.

The worker's strict JSON request adds default-false check_only; existing requests
still execute. Boa Script::parse is used only inside the resource-limited child.
Syntax-only validation is Linux-only until equivalent isolation is implemented.
Store/crash/tampering/syntax, authenticated HTTP, real/controlled-core and a seventh
bundle browser command workflow verify the adaptation. The global browser editor,
authoritative settings/DNS/TUN/final cleanup and automatic backups remain pending.


## Browser global enhancement controls adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src/pages/profiles.tsx` global Merge/Script ProfileMore controls and
`src/components/profile/profile-more.tsx` read/edit/save/reset/log behavior.
The new GlobalEnhancements/GlobalEditor components in web/src/main.tsx are service
UI adaptations, not copies of the Material UI/Monaco/Tauri implementation. They
use authenticated global read/set/reset commands through the existing shared
request/command lock and keep global rows out of base profile cards.

Raw source is loaded before editing; failed saves retain the user's input and
surface backend errors. Existing Logs exposes worker console diagnostics. Exact
UTF-8 source limits are checked before command submission. Source read failures
leave an explicitly unsaved empty draft with retry/replacement/reset recovery.
Reset uses inline confirmation and invokes the backend to restore the upstream
template; no template copy is embedded in the browser. Opening another editor is
disabled while the current editor is open, and mutations honor shared busy state.
Cancel changes no persisted state.

The panel documents active application, standalone deferred script execution,
stopped-core behavior and upstream repeated-global fallback. Full-width grid and
wrapping actions support mobile editing/reset. A new eighth release-bundle
Chromium workflow verifies save/read/cancel/failure/reset, console/runtime views,
source preservation across restart, size limits, read-error retry, mobile layout,
stopped/no-current operation and the restored MVP. Authoritative settings/DNS/TUN,
final cleanup and the broader desktop settings/resources remain pending.

## Service settings foundation adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src-tauri/src/config/clash.rs` typed port/mode/log/LAN/IPv6 settings and
`src-tauri/src/enhance/mod.rs` merge_default_config, CONTROL_PLANE_KEYS,
AuthoritativeFields and enhance ordering. The new config/settings.rs is a
service adaptation, not a copied Tauri singleton or desktop settings schema.

The initial versioned subset uses explicit optional values, retains port zero as
disabled, validates values without upstream numeric truncation, rejects controller
fields, and initializes an empty mapping rather than desktop listener defaults.
Only explicit settings have authority; absent fields preserve current MVP behavior.
GenerationPlan separates global merge from sequence output to inject settings at
the upstream stage. Final enforcement follows upstream end-of-chain restoration;
discard diagnostics use existing bounded service logs instead of GUI notices.
Candidate staging also covers standalone imports/edits and bootstrap. Startup
restores committed runtime snapshots without rerunning enhancements. Settings
storage is private, bounded and atomically replaced under the directory lock.
Online update coordination, DNS/TUN and full control-plane authority remain pending.

## Online service settings transaction adaptation

Reference: the same pinned revision, `src-tauri/src/cmd/clash.rs`
patch_clash_config and `src-tauri/src/feat/config.rs` patch_clash draft/apply/discard
flow. The service uses a typed whole-subset replacement instead of copying desktop
commands or mutable GUI singletons. Settings reads/updates serialize through the
existing actor; changes regenerate active subscriptions or update committed
standalone runtime, preserving validation, stopped-state behavior and rollback.
A private settings journal coordinates publication with the runtime revision commit
and enables startup/admission recovery at either side of publication. No-runtime
updates are standalone atomic settings writes. Management authentication and strict
command decoding remain shared with other commands. Storage/actor recovery, HTTP
contract, real-Mihomo transaction tests and a ninth release-bundle browser workflow
verify the adaptation. DNS/TUN, complete
desktop settings and the browser settings editor remain pending.

## Browser runtime settings editor adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src/components/setting/setting-clash.tsx` field controls and
`src/components/setting/mods/clash-port-viewer.tsx` port editing. The service-native
web/src/settings.tsx adapts the already migrated typed runtime subset to optional
inheritance values and whole-subset authenticated replacement. It does not copy
Material UI, Tauri hooks, local system-proxy actions or controller settings.

The editor preserves explicit zero/false, validates ports, keeps rejected drafts,
confirms reload/clear actions and independently reads back after any save result.
Uncertain outcomes gate resubmission; reconciliation can show a committed draft
even when the mutation response reports an error. Unknown schemas/fields/types
cannot become implicit empty replacements. Read cancellation/session expiry follow
the existing browser ownership model. A saved-settings summary, responsive layout
and no-config/standalone/stopped guidance accompany the controls. The existing MVP
workflow adds no-config editing and a tenth Chromium workflow exercises active
save/clear, failures, readback, mobile layout and restart. DNS/TUN, complete desktop
settings/resources and upgrades/backups UI remain pending.
The Rust static-asset navigation allowlist adds `/settings` for direct GET/HEAD
visits while retaining the scoped SPA fallback and authenticated API boundaries.

## Typed DNS/TUN settings authority adaptation

Reference: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b,
`src-tauri/src/config/clash.rs` IClashDNS/IClashTUN,
`src-tauri/src/constants.rs` tun::GUI_KEYS and
`src-tauri/src/enhance/mod.rs` is_set, merge_dns_config and AuthoritativeFields.
The new `config/settings/network.rs` is a strict service subset of these fields,
with typed mode/stack enums, optional values, Linux-only auto-redirect, nonzero
16-bit MTU and nonempty device names. DNS ipv6 is supported alongside the other
basic fields. Provider resolver/policy fields, fallback-filter and hosts are not
accepted as settings yet; source values remain intact when unowned.

DNS override selection retains upstream is_set semantics: false, null, blank text
and empty lists inherit source values, while true/nonempty values have authority.
TUN explicitly saved false and empty lists remain authoritative. Settings merge
shallowly into each section and final enforcement restores only owned keys after
global/profile enhancements, preserving custom subscription fields. Diagnostics
identify owned paths such as dns.nameserver and tun.enable. Existing staged
validation and settings/runtime journals handle both new sections without a
second writer or transaction format change. Empty defaults preserve prior MVP
behavior and the current scalar editor refuses nested snapshots without dropping
data. Tests cover strict decoding, ownership, restart, journal decisions and real
Mihomo validation rejection. Upstream use_tun DNS derivation, provider-specific
override confirmation/auto-disable, host DNS changes and the DNS/TUN UI remain
unmigrated; this increment does not claim the complete upstream network workflow.

## Pure TUN/DNS derivation and candidate phase adaptation

Source revision: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
`src-tauri/src/enhance/tun.rs` →
`crates/headless-core/src/enhance/tun.rs` retains the pure mapping behavior:
TUN enable ownership, fake-IP DNS enable/IPv6 derivation, missing default mode
and IPv4/IPv6 ranges, redir-host preservation and disabled-TUN behavior.
The macOS AsyncHandler/public-DNS modification/restoration is not copied.
`src-tauri/src/enhance/mod.rs` ensure_fake_ip_range6 is adapted to repair missing,
null, blank or non-string IPv6 ranges after the optional DNS settings stage.

RuntimeSettings::prepare preserves upstream phase order: ordinary/TUN settings →
use_tun → DNS settings → IPv6 range repair → global/profile overrides → final
explicit authority. Headless optional inheritance invokes use_tun only when
tun.enable is explicitly saved; a source-owned enabled TUN is not implicitly
promoted to service ownership. Derived DNS keys are not automatically captured
as authoritative settings, matching upstream's separate DNS-page ownership.
The service introduces a private Raw/Enhanced candidate enum to prepare standalone
and bootstrap inputs once while avoiding a second derivation after manual scripts.
It adds no YAML/API fields or persistent transaction format. Committed snapshots
are restored without regeneration as before. Pure tests and real-Mihomo stopped
candidate validation exercise these semantics; privileged native TUN operation,
host DNS management and provider DNS conflict confirmation remain unverified or
pending integrations.

## Provider DNS protection and coordinated preference commit

Source revision: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
`src-tauri/src/config/dns.rs` → `headless-core/src/config/dns.rs` retains the three
original nonempty provider resolver/policy fields, recursively sorted JSON,
SHA-256(profile UID + NUL + JSON), requested/enabled distinction and per-profile
preference semantics. Ring supplies SHA-256 using the version already pinned for
the service; a fixed compatibility digest verifies the adaptation.
GenerationPlan captures the digest from raw subscription mapping before
sequence/global/profile enhancements, so generated policies do not become new
confirmation challenges. No Tauri notice or global draft singleton is copied.

`src-tauri/src/feat/dns.rs` informs active-profile-only set_profile_dns and
confirmation_required/applied outcomes. The actor validates source/UID before
mutation, keeps confirmation digests only in a session HashMap, preserves them
across switches and restores old confirmations on failed candidate commits.
Settings retain a strict optional profile_dns map of enabled flags; generic
set_settings replaces runtime fields while preserving these preferences.
Only the affected profile is automatically disabled. DNS-page fields and their
authority are suppressed while protected; independent TUN derivation remains.

The service strengthens upstream's post-runtime preference write by including
automatic preference changes in the existing settings/runtime journal, alongside
refresh or enhancement journals when needed. Runtime manifest commit decides all
publication/recovery and only accepted changes invalidate cached confirmation or
emit a bounded auto-disabled diagnostic. Inactive enhancement checks do not
publish preference changes. Read/set commands share authenticated actor admission.

Headless recovery restores committed YAML without running scripts: restart clears
session confirmations immediately for future generation, but does not rewrite a
previously confirmed committed snapshot. Reselect/refresh/edit generates a new
protected candidate and commits the disabled preference. This explicit recovery
adaptation differs from the desktop startup enhancement pass. Tests verify source
scoping, strict preferences, HTTP auth/decoding, switch/restart behavior, failed
confirmation and simultaneous refresh/settings publication recovery. Dedicated
DNS/TUN UI, policy/hosts settings, stale preference cleanup on profile deletion
and privileged native TUN routing remain separate integrations.

## Browser DNS/TUN settings and provider confirmation adaptation

Source revision: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
`src/components/setting/mods/dns-viewer.tsx` and `tun-viewer.tsx` inform the
supported DNS/TUN field labels and inheritance distinctions. The React service
editor is an independent adaptation using the already migrated typed subset and
authenticated actor commands, without desktop invoke/hooks or host DNS changes.
It preserves explicit false, empty strings, empty arrays and empty section objects
through whole-runtime replacements; per-string source controls distinguish absent
from empty. JSON string-array inputs preserve commas and embedded newlines without
lossy upstream comma splitting. Unknown nested fields fail closed.

The separate active-profile panel uses profile_dns/set_profile_dns, independent
readback and explicit confirmation_required handling. UID/component lifetime and
runtime generation/revision changes and WebSocket disconnection invalidate displayed questions; the backend
remains authoritative for session/source validity. Reconnect rereads permission even when revision/generation are unchanged. Refresh/restart never confirms
a new digest automatically. The panel distinguishes saved preference, generation
permission and committed runtime, documenting snapshot recovery behavior.
Policy/hosts/fallback-filter editing, native TUN operation and final group/LAN
cleanup remain pending.

## Final LAN binding and proxy-group cleanup adaptation

Source revision: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
`src-tauri/src/enhance/mod.rs` is_loopback_bind_address,
is_ipv4_shorthand_loopback, ensure_lan_bind_address and cleanup_proxy_groups
are extracted into headless-core/src/enhance/finalize.rs. Standard-library String
replaces smartstring in temporary name sets; membership and filtering remain the
same. Existing field::use_sort supplies upstream final field order.

After restored runtime authority, allow-lan=true widens only an explicitly
loopback bind-address to `*`: localhost, parsed loopback IPv4/IPv6 (including
bracketed IPv6), and bounded IPv4 shorthand are retained. Missing/custom/nonstring
bindings and disabled/inherited allow-lan do not acquire a new binding. This
changes Mihomo proxy listeners, not the service's management listen/public-origin
arguments or private controller transport.

Group cleanup gathers proxy, group, provider and built-in names. Invalid `use`
entries are removed. A surviving valid provider preserves dynamic proxy names;
otherwise unknown string proxy references are removed. Stable ordering, duplicates,
nested groups, upstream built-ins and malformed nonstring proxy entries are
preserved. Empty groups receive no invented DIRECT fallback; malformed structures
are left for Mihomo validation. No provider network fetch is introduced.

The service invokes finalization only at its shared candidate staging boundary,
after all merges/scripts and final authority, before store staging and `mihomo -t`.
Intermediate mappings and downloaded/raw subscription files remain unchanged.
Bootstrap, standalone edits/overlays, profile select/refresh, enhancement edits
and settings transactions share this path. Already committed startup snapshots
are not regenerated. Pure tests retain and extend upstream group/LAN cases;
real-Mihomo tests cover ordering, late script-created references, validation
rollback, stopped application and recovery. Full resources, DNS policy/hosts,
raw editing/cascade deletion and native TUN remain incomplete.

## Original profile YAML editing adaptation

Source revision: Clash Verge Rev b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
`src-tauri/src/cmd/profile.rs::read_profile_file` and
`src-tauri/src/cmd/save_profile.rs::save_profile_file` inform UID-based raw reads,
original-YAML validation for inactive as well as active profiles, followed by
current-profile enhancement/validation/application and restoration on failure.
Typed auxiliary/global editors remain separate; raw editing permits only local
and remote base items. Desktop invokes, notice targets and automatic backups are
not migrated by this increment.

The service strengthens in-place writes/restoration with immutable source files,
an opaque catalog file revision and compare-before-save. profile_raw returns
uid/revision/yaml; set_profile_raw requires the previously read revision. The
service first parses/bounds the raw mapping and enforces its private controller
boundary, validates an immutable runtime-store validation copy with Mihomo, then
uses the enhanced candidate workflow for the active UID. Inactive raw updates
validate with Mihomo but do not run linked scripts or change the active runtime.
No failed candidate is published before these checks. Exact raw text, including
comments/BOM/line endings, is saved without reserialization. Validation revisions
use existing immutable staging retention; file garbage collection remains separate.

The existing profile-refresh.yaml journal gains optional kind: raw_edit. Legacy
refresh journals omit kind and retain their existing recovery rules. Raw edits
preserve every catalog metadata field except the immutable file pointer; recovery
rejects raw journals that change metadata. Active raw/runtime/settings publication
follows the committed runtime revision, inactive raw publication the catalog
pointer. Existing startup/admission/rollback recovery handles both kinds, including
provider DNS auto-disable in the settings journal. Changed file pointers also
invalidate already downloaded refreshes through the existing stale-source guard.

The browser raw editor independently reads and reconciles saves, preserves failed
drafts, requires explicit reload after a version conflict, and never adopts a
changed external revision as the base for an old draft. Lost responses are
reconciled without another mutation. Dirty reload/close requires explicit discard;
owned reads cancel on unmount and authentication expiry logs out. Remote refresh
intentionally replaces manual edits. Cascade deletion, stale preference cleanup,
raw revision garbage collection and automatic backups remain pending.


## Auxiliary cascade deletion and deleted DNS preferences

Adapted from pinned upstream `src-tauri/src/config/profiles.rs::plan_delete_item`
and `src-tauri/src/cmd/profile.rs::delete_profile`. Deletion now removes the base
and its exclusive merge/script/rules/proxies/groups rows in one catalog rename.
The service deliberately protects active/current profiles, shared links, reserved
Merge/Script/Rules/Proxies/Groups rows and files referenced by survivors. It does
not automatically select a fallback profile. Missing auxiliary rows are tolerated
for repair; wrong-type or nested auxiliary rows are rejected.

The private deletion journal adds schema version 2 and up to five auxiliary
UID/file pairs. Version 1 single-file journals remain readable. Recovery aborts
uncommitted plans, or cleans persisted DNS preferences and files after catalog
commit. The journal remains until both cleanup phases finish. Settings cleanup
is atomic and leaves runtime settings/revision unchanged; session confirmations
are removed in the actor. Startup also prunes preferences from older deletions
after recovering runtime/settings/catalog journals. Original immutable revisions
and orphan auxiliary files remain subject to separately pending garbage collection.

## Automatic auxiliary defaults during service imports

Adapted from pinned `src-tauri/src/config/prfitem.rs::from_local/from_url`,
`from_merge/from_script/from_rules/from_proxies/from_groups`, and the five
`ITEM_*` templates in `src-tauri/src/utils/tmpl.rs`. New service imports create
only missing per-profile links: an empty comment-only Merge, the identity Script,
and three empty prepend/append/delete sequences. Existing valid shared/reserved
links and remote download metadata remain intact. Newly linked identity scripts
make the global script run once; legacy unlinked/cleared profile stages retain
upstream global fallback and double execution.

Unlike upstream's separate auxiliary appends, the service allocates files and
records a bounded private profile-import.yaml journal before writing content.
One catalog rename publishes the base and all newly owned auxiliaries. Startup
and command admission either retain the exact committed rows/content or abort
and remove only allocated, unreferenced files. Reused links are fingerprinted;
unsafe paths, symlinks, mismatched hashes and partial catalogs fail explicitly.
Import remains separate from activation and does not alter runtime/PID. Local
file, YAML, remote and CLI paths share this workflow. Existing catalogs are not
backfilled. Low-level ProfileStore import_local/import_remote primitives remain
available for legacy catalog migration; service calls use the *_with_defaults
transactional methods. Retired immutable revisions remain pending garbage collection.


## Managed-core subscription proxy transport

Adapted from pinned `src-tauri/src/config/prfitem.rs::from_url` (self_proxy priority
over with_proxy), and `src-tauri/src/utils/network.rs::create_client`'s localhost
HTTP proxy branch. The new service uses the actual running core's Mixed/HTTP
listener rather than the desktop desired-port singleton, verifies committed ports,
and supports proxy authentication from the private committed runtime snapshot.
Mihomo's public config response exposes authentication usernames, not passwords;
these are compared before private credentials are attached to the loopback proxy.

No arbitrary client-supplied proxy URL or implicit environment proxy is accepted.
A lifecycle/configuration snapshot guards route resolution and cancels stale
in-flight requests. Network tasks stay outside the lifecycle actor and retain the
existing semaphore, bounded body/time/redirect behavior, shutdown cancellation and
source metadata compare-and-swap guards. Persisted self_proxy and strict metadata
patches are exposed in import and edit UI. Missing/false retains direct behavior;
core unavailability returns an explicit error without direct fallback. System
proxy discovery, SOCKS-only transport, TLS fallback/bypass and scheduling remain
pending. Real-Mihomo integration exercises an authenticated ingress and controlled
HTTP upstream tunnel, so a synthetic subscription hostname succeeds only through
that proxy chain; origin/upstream headers are checked for credential separation.


## System-proxy remote subscriptions in the headless service

Adapted from pinned `src-tauri/src/config/prfitem.rs::from_url` (self_proxy before
with_proxy before direct) and `src-tauri/src/utils/network.rs::create_request_with_tls_mode`
(system Sysproxy discovery, disabled/unavailable proxy permits direct). The Linux
service uses Reqwest 0.13.5 / Hyper-util 0.1.20's environment-based system matcher
instead of querying a desktop Sysproxy singleton. These already locked libraries
provide protocol routing, HTTP basic authentication, HTTPS CONNECT and NO_PROXY;
Reqwest's system-proxy feature is explicitly retained. Windows/macOS native discovery
code remains available in those libraries but is not newly runtime-verified.

Only with_proxy true and self_proxy false enables default discovery. Direct and
managed modes call no_proxy before any explicit managed route. Strict boolean
options are persisted, merged by metadata patches and used on refresh. No new
client-supplied proxy URL is accepted. Bounded validation of effective environment
variables rejects malformed/unsupported proxy endpoints using only variable names
in errors. Uppercase precedence and CGI suppression follow the inspected locked
matcher. The service additionally treats a NO_PROXY `*` entry as global bypass
for IP literals as well as hostnames, correcting the locked matcher's domain-only
wildcard behavior. An unavailable selected endpoint fails instead of retrying directly;
missing configuration and explicit NO_PROXY retain normal direct behavior.

Process-isolated tests avoid unsafe/global Rust environment mutation and exercise
actual service imports/refreshes through controlled HTTP proxies, credentials,
redirects to bypassed origins, HTTP/HTTPS/ALL precedence, HTTPS CONNECT, absent and
empty configuration, CGI/bypass, metadata conflicts, restart persistence, body/
time/concurrency bounds and shutdown cancellation. A real Mihomo workflow verifies
managed priority, authenticated local ingress and that system refresh remains
pending across core stop then succeeds. Provider/proxy requests never carry the
management token. Browser import/edit choices preserve failed drafts and explicit
false mode switches. Linux desktop gsettings, PAC/WPAD, SOCKS, TLS fallback/bypass,
scheduling and other-platform runtime verification remain separate work.


## Subscription TLS verification and static-root fallback

Source: `src-tauri/src/utils/network.rs` at pinned commit
`b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`: `TlsRootMode`,
`should_retry_with_static_webpki_roots`, `is_legacy_tls_protocol_error`,
`context_reqwest_error`, `build_client`, and `get`. Destination:
`service/src/remote/tls.rs` and `service/src/remote.rs`. This increment supersedes
previous TLS-fallback/bypass pending notes above; other pending workflows remain.

Retained behavior: platform verifier first, one static-root retry for the same
TLS/certificate/revocation keyword classification, protocol-version errors excluded,
explicit per-subscription certificate and hostname bypass with no root retry,
TLS 1.2/1.3 and proxy priority. Reqwest's current public `tls_certs_only` API uses
the already locked `webpki-root-certs` 1.0.9 DER Mozilla root bundle, corresponding
to upstream's WebPKI trust anchors, instead of injecting an older preconfigured
Rustls/ring client. The normal platform verifier remains unchanged; no extra
production cryptography provider or package version is introduced.

Service adaptations: both attempts consume one total timeout; the managed route
and private credentials are borrowed across attempts, and system proxy discovery
is still opt-in. URL-stripped transport errors preserve TLS error chains and the
legacy-protocol diagnostic. HTTP/body/YAML failures never relax verification.
Strict import/edit schemas persist the existing PrfOption boolean without touching
linked enhancements. Refresh's full-option guard supersedes old downloads when
this option changes. Web controls explain the effect and retain failed drafts.

Ephemeral private OpenSSL certificates plus Rustls/Tokio HTTPS servers exercise
untrusted/wrong-name failure, static retry, explicit bypass, total deadline,
protocol-version alerts, body/status/size failures, HTTP-to-HTTPS redirects,
platform custom CA, authenticated CONNECT routing, persistence, stale refresh and
shutdown. These fixtures use only loopback addresses and delete their keys; real
Mihomo integration additionally verifies managed priority and stop cancellation.
Rustls and tokio-rustls test dependencies reuse already locked versions.


## Scheduled subscription updates

Source: `src-tauri/src/core/timer.rs` (`TaskSchedule::new`, `gen_map_from_items`,
`apply_timer_map`, `mark_task_running`, `finish_task`, `interval_duration`) and
`src-tauri/src/feat/profile.rs::should_update_profile`, pinned commit
`b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b`. Destination:
`service/src/scheduler.rs` (a private core_manager module) and scheduled refresh
adapters in `service/src/core_manager.rs`. This supersedes previous scheduling
pending notes above.

Retained rules: positive minute intervals, allow-auto default true, updated-plus-
interval first deadline, overdue immediate work, missing/future timestamps waiting
one interval, per-UID running guard, retirement across disable/re-enable, and a full
interval after completion even on failure. Interval multiplication saturates.
Only remote rows with UID/URL are registered, matching the actual upstream update
eligibility rather than scheduling auxiliary/local rows that its update function
would skip. Manual refresh still overrides disabled automatic policy.

Service adaptations: a recovered profile watch replaces the singleton/unbounded
command channel and desktop initialization/tray/notification calls. A monotonic
in-memory map with checked Instant arithmetic replaces DelayQueue; no new runtime
package is needed. JoinSet owns at most four workers, sharing existing download
admission with manual work. Watch changes include updated timestamps, so successful
manual refresh resets the pending first deadline. In-flight interval changes apply
at completion; failures retain a full in-memory interval without persisting a fake
successful timestamp. Managed timers wait through transitional core phases.

Automatic refresh reuses the existing transactional/manual path with policy checks
before and after download admission and at actor admission. File/URL/full-option
CAS, TLS/proxy priority, bounds, active apply/rollback and recovery remain intact.
A weak actor command sender avoids an idle scheduler keeping the manager alive;
shutdown signals cancel/drain owned tasks and completion is awaited alongside the
actor. Generic bounded log messages omit private provider input.

Tests cover clock boundaries, large intervals, filter rules, retirement and manual
reset; loopback service workflows cover overdue startup, persisted fresh timestamps,
failed-run backoff, interval edits, disabled manual updates, stale downloads,
shared admission and shutdown/drop ownership. A paused Tokio test drives the actual
60-second timer without external processes or real minute sleeps. A real Mihomo
workflow verifies scheduled managed-route semantics, hot application, node
record restoration and invalid-candidate rollback. Development-only Tokio test-util
reuses the existing locked dependency; production requires no timer process.


## Stable core metadata and compressed-package preparation

Source: src-tauri/src/feat/core_upgrade.rs at pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b: resolve_latest_version,
asset_base_name, package_url, download_package and stage_core. Destination:
service/src/core_release.rs and CoreManager/management command adapters.
Retained behavior includes fixing the version before downloading, Linux amd64-v2
asset naming, stable version-specific GitHub URLs, 20-second metadata and
300-second/64-MiB package bounds. Desktop progress/notifications and globals are
not copied. This increment covers compressed preparation only, superseding earlier
blanket core-upgrade pending status without claiming switching is implemented.

The service uses the official public release API to require exactly one uploaded
asset and its size/digest/browser_download_url; see the
[GitHub release assets API](https://docs.github.com/en/rest/releases/assets?apiVersion=2022-11-28).
SHA-256 verification and strict tag/target/URL checks precede private atomic cache
publication. No caller-provided source/checksum/path is accepted. Typed readback
revalidates cached files; temporary cleanup handles failed/cancelled downloads and
interrupted preparation. Authenticated commands add one immediate network admission
slot and shutdown cancellation outside the lifecycle actor. No new dependency is
introduced; existing Reqwest/ring provide transport/digest primitives.

Loopback fixtures exercise metadata/status/bounds, invalid stable tags, missing/
duplicate assets, integrity failures, forbidden and allowed redirects, private
cache reuse/restart/tampering, symlinks, paused-clock timeout and cancellation.
A real-Mihomo gzip fixture independently decompresses the downloaded package for
byte comparison and verifies the installed core remains unchanged. This is test
verification, not a production decompressor. Managed/system proxy routes and TLS
static-root retry, upstream gzip decoding/executable probes, actor replacement/
rollback, Alpha and other-platform resource runtime validation remain pending.


## Bounded staged core extraction and compatibility probes

Source: src-tauri/src/feat/core_upgrade.rs::stage_core, unpack and read_core_version,
pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Destination:
service/src/core_stage.rs (private core_release module), version probe in validation.rs,
and actor/management adapters. Retained behavior includes gzip extraction, private
staging, executable permission/fsync and requiring the exact reported version before
publication. Live-core replacement/rollback remains a separate pending increment.

The service pins flate2 1.1.10 with default features disabled and rust_backend;
Cargo.lock pins its pure Rust miniz_oxide/CRC/Adler dependencies. No system gzip or
new native compression package is required in production. The
[flate2 single-member decoder](https://docs.rs/flate2/1.1.10/flate2/bufread/struct.GzDecoder.html)
allows checking unread input after EOF; the adaptation rejects concatenated members
and trailing bytes, checks CRC/truncation, caps uncompressed output at 128 MiB and
checks a cooperative 15-second clock plus shutdown between bounded chunks.

Additional service checks require Linux x86_64 ELF headers, bounded async -v and
-t processes with cancellation/kill/reap, and actor-serialized configuration snapshots.
Configuration testing uses a disposable data directory and bounded copies of known
Geo files. It does not claim full provider/resource staging or successful activation.
Probe diagnostics are suppressed in management errors. Executable/config hashes
are rechecked after probing, recorded with the compressed source identity, atomically
published and revalidated by restart readback. Admission remains actor-owned after
a caller disconnects. Existing status, runtime, selections and live executable stay
unchanged. Tests cover malformed archives/CRC/members/limits/ELF, output/time/cancel
bounds, cache permissions/links/tampering, actual Mihomo compatibility failures and
successful actor staging with a running core and restart proof readback.


## Actor-owned core activation and interrupted-switch recovery

Source: src-tauri/src/feat/core_upgrade.rs::upgrade_core and StagedCore::publish,
pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Destination:
service/src/core_upgrade.rs, activation/admission adapters in core_manager.rs and
management commands. Retained behavior fixes a validated target before replacement,
uses a same-filesystem atomic publish, retains a restorable previous core, checks
actual restart outcome and restores/restarts the old core after replacement failure.
No desktop approved-copy/elevation, singleton, tray/notification or Mihomo self-upgrade
API is introduced. Upstream latest/no-op/force command compatibility and upgrade UI
remain a following increment; this explicit stage-ID command always replaces.

Private bounded copies replace upstream's hard-link backup to allow independent
hash checking and preserve ordinary file mode. A versioned journal plus atomic
installation receipt adds crash recovery: pending means restore old bytes/receipt,
committed means complete new metadata/cleanup. Startup recovery precedes bundle
seeding; actor admission and retries recover before lifecycle changes. Unknown
files/links, malformed records or required-backup/live-hash conflicts fail closed.
The actor requires current configuration identity and reruns candidate validation,
then checks live version/ports twice before commit. Stopped-mode activation briefly
starts the candidate to verify runtime behavior and returns to stopped state.
Existing revision/profile/settings/node stores and bounded node restoration are reused.

Linux parent lifetime is bound for ordinary core and validator children using
[PR_SET_PDEATHSIG](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)
with SIGKILL and a getppid race check in async-signal-safe pre_exec calls. This is
Linux-specific, refers to the creating parent thread, and does not replace normal
SIGTERM/reaping. Candidate copies have no privilege xattrs; activation rejects
capability-bearing/set-ID previous cores rather than silently losing privileges.
Native privilege integration and other-platform recovery remain pending.

Tests cover both journal phases, partial preparation/rollback checkpoints, mode and
receipt restoration, conflicting/corrupt data, links, failed stopped activation,
actual running/stopped Mihomo replacement, preserved node/config/profile state,
stale candidate rejection, startup-failure rollback, shutdown without restart,
changed bundle seed preservation, and SIGKILL of a real management process during
replacement followed by candidate termination and successful startup rollback.
No new package dependency is needed.

## Stable force/no-op upgrade command and browser workflow

Source: src-tauri/src/feat/core_upgrade.rs::upgrade_core, CoreUpgradeReport and
src-tauri/src/cmd/clash.rs::upgrade_clash_core at pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. The service command retains required
boolean force, latest stable resolution, same-version no-op before download and
the upgraded/from/to report. Force follows verified staging and actor-owned
activation even for the same version. The new `/core` React page uses authenticated
commands, installed version/receipt readback, busy controls, force confirmation
and retry/reconnect reconciliation; it introduces no desktop IPC dependency.

Adaptations pin the official API asset metadata/digest across discovery/download,
retain one upgrade admission slot, keep network work outside the lifecycle actor,
and recheck installed version/current configuration at final actor admission.
Verified immutable compressed caches can be reused. Existing durable activation
and running/stopped semantics are unchanged. The wrapper deliberately fails on an
unreadable/empty old executable until broken-core repair has a durable recovery
design. Upstream managed/system/direct route fallback, static-root TLS retry,
Alpha assets, native privilege and other-platform integrations remain pending.

Tests cover pinned metadata when latest moves, pre-publication cancellation,
same-version no-op preserving PID/inode/revision/receipt, forced failed-candidate
rollback and real running/stopped same-version reinstall. Browser transport
fixtures check duplicate suppression, no-op/result/error display and force
confirmation; forced success delegates to real authenticated stage/activation,
including persistent receipt readback after service restart. Separate official
release smoke exercises the actual end-to-end wrapper.

## Core-download routing and verified TLS-root retry

Source: src-tauri/src/feat/core_upgrade.rs::resolve_latest_version/download_package
and src-tauri/src/utils/network.rs TLS/proxy policy, pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Destination: core_release.rs,
core_release_transport.rs and actor-backed resolution in core_manager.rs. Retained
behavior tries managed/system/direct discovery in order and downloads with the
successful policy. Shared private remote environment/TLS helpers keep platform-first
verification and selective static Mozilla-root retry; no certificate bypass is added.

Managed actual ports/authentication use a generation/PID/revision guard, cancellation
on lifecycle changes and identity checks before cache reuse/publication. Subscription
downloads reuse this guard. System discovery retains validated environment, NO_PROXY/
CGI and native library support; Windows/macOS runtime checks, SOCKS/PAC remain pending.
Proxy secrets/addresses stay in private ephemeral objects; logs contain policy names.
Metadata has 20 seconds per policy and package one 300-second budget including root
retry. Existing redirect/size/hash/version/config protections remain. Package failure
does not select a different route or publish partial data.

Fixtures cover authenticated managed metadata/package affinity, metadata fallback,
package status/integrity rejection without route switching, generation/revision/stop/
watch-close cancellation/cleanup and wrong-host/untrusted TLS rejection in both modes.
Process-isolated children exercise system authentication, NO_PROXY global/IP bypass,
CGI and direct fallback without changing the parent environment. Existing subscription
routing/TLS and real activation tests remain applicable. No dependency is added.

## Broken managed-core repair and durable rollback

Source: `src-tauri/src/feat/core_upgrade.rs::upgrade_core`, pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Upstream treats failed version readback
as a repair request and uses atomic replacement to avoid truncating running inodes.
Destination: resources.rs, core_manager.rs, core_upgrade.rs and web/core-upgrade.tsx.
The service admits safe broken existing files for management startup and reports
`unknown`; the force/no-op adapter always prepares unknown versions. Unlike upstream's
best-effort rollback for readable old cores, repairs preserve even unreadable/empty
old files with a private hard-link backup and a schema-2 identity journal. Normal
schema-1 digest transactions remain compatible. Failed/crashed activation restores
the original inode/mode and a valid previous receipt, even when that receipt no
longer verifies against the broken file. Commit still requires candidate digest,
version/configuration probes and live health/port verification.

The page retains separate version/receipt readback so an unverified previous
receipt does not block an admitted repair. Tests cover pending/committed/rollback
boundaries, unsafe files and identity conflicts, failed repair/retry, actual
Mihomo empty/unreadable/execute-only repair, saved node restoration, receipt
persistence, process SIGKILL recovery and browser repair readback. Paths, package
and authentication stay actor/private-service owned; no new dependency is added.
Alpha/native privilege and other-platform repair remain pending.

## Alpha release metadata and compressed preparation

Source: `src-tauri/src/feat/core_upgrade.rs::resolve_latest_version`,
`package_url`, `asset_base_name` and `download_package`, pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Destinations: core_release.rs,
core_manager.rs and management/mod.rs. Retained Alpha behavior uses the fixed
`Prerelease-Alpha` tag, Linux amd64-v2 variant and pinned version/package name.
The service reads one official API metadata snapshot instead of mutable version.txt,
requiring published prerelease status, a unique ordinary gzip asset, declared size
and SHA-256. Supported Alpha versions are `alpha-<7..40 lowercase hex>`; Go-specific
variants are ignored. Optional pinned requests must match the current Alpha asset.

Route affinity, platform/static trusted roots, bounds, admission and cancellation
reuse the stable pipeline. Cache IDs split the last digest separator to retain
hyphenated Alpha versions without changing existing stable manifest schemas.
Fixtures cover channel rejection, invalid/ambiguous metadata, moving release
snapshot integrity, cancellation and restart readback. Preparation never touches
the live core; executable staging deliberately rejects Alpha before unpack/probe.
Alpha activation/receipts/force-no-op/Web and other targets remain pending.
No dependency is added.

## Alpha executable, version and configuration staging

Source: `src-tauri/src/feat/core_upgrade.rs::stage_core`, `unpack` and
`read_core_version`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Destination: core_stage.rs and the actor staging/activation boundary in
core_manager.rs. Alpha reuses the verified Linux gzip/ELF pipeline, exact version
probe and isolated configuration validation already adapted for stable candidates.
Hyphenated Alpha stage IDs split the package and configuration digests from the
right; existing stable schema-1 manifests remain compatible. Immutable proofs are
published only after execution-time executable/YAML rehashing. Readback rechecks
package, executable, config, permissions and manifest identity after restart.

Fixtures cover Alpha successful/cached staging, private Geo validation resources,
wrong version/configuration, executable/YAML mutation, cancellation/reaping of both
probe phases, tampered readback and actor activation gating before new probes or
journaling. Alpha activation remains explicitly rejected until durable receipt and
rollback support is implemented. Existing stable lifecycle/upgrade behavior and
MVP proxy traffic remain supported; no dependency is added.


## Durable Alpha activation and installation recovery

Source: `src-tauri/src/feat/core_upgrade.rs::upgrade_core`, `StagedCore::publish`
and `read_core_version`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Destinations: core_manager.rs and core_upgrade.rs. Alpha now enters the existing
service-owned stable activation transaction after fresh snapshot/version/config
validation. Exact runtime version and actual proxy ports gate the durable commit;
failed activation, shutdown and pending crash recovery restore the previous core
and receipt. Running/stopped semantics and profile/node selections are retained.
The service's durable journal strengthens upstream's sidecar rollback; it never
requests native elevation or updates an unrelated administrator-approved service.

Receipt IDs parse digest suffixes from the right using the shared bounded
stable/Alpha version grammar. Normal schema-1 journals/receipts and schema-2 repair
journals retain their existing layouts. Alpha-to-stable, stable-to-Alpha and
Alpha-to-Alpha rollback/commit, broken Alpha inode/receipt recovery, malformed IDs,
failed readiness, shutdown and Alpha candidate SIGKILL recovery are covered.
Official Alpha activation is exercised with an isolated real-node subscription.
Channel-aware Alpha force/no-op, Web controls and other targets remain pending.
No dependency is added.


## Alpha force/no-op adapter and channel-aware Web workflow

Source: `src-tauri/src/feat/core_upgrade.rs::upgrade_core` and
`resolve_latest_version`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Destinations: core_manager.rs, management/mod.rs and web/src/core-upgrade.tsx.
Retained semantics: discover the selected channel, skip an unchanged installed
version unless forced, repair an unreadable previous core, preserve runtime mode
and restore the previous core on failure. Explicit `upgrade_alpha_core` and stable
`upgrade_clash_core` commands replace upstream desktop core-name selection without
introducing a persistent setting or accepting arbitrary release sources.

Both commands reuse verified route-affine metadata/preparation, single admission,
shutdown cancellation and durable actor staging/activation. A second actor no-op
check handles changes while downloading. Browser channel selection clears stale
results, locks during requests and displays actual installation/repair readback.
Fixtures cover unavailable-package no-op, forced failure rollback, authenticated
request bounds, both channel controls and real executable/receipt recovery after
restart. The official Alpha wrapper is separately exercised with real-node traffic,
including force, unchanged-version no-op and unknown-core repair. Other platforms,
Alpha bundle seeds and cached-candidate garbage collection remain pending.
No dependency is added.


## Bounded local backup export and service backup models

Source: `src-tauri/src/core/backup.rs::create_backup` and
`src-tauri/src/feat/backup.rs::LocalBackupFile`, pinned commit
b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b. Destinations:
crates/headless-core/src/backup.rs, service/src/backup.rs, core_manager.rs and
management/http.rs. Retained behaviors are a Stored ZIP container, raw profile
contents and configuration export, with application-specific credentials excluded.
Service metadata retains filename/content length but replaces host paths and
human-local timestamps with portable schema/digest/Unix-time metadata.

The actor-owned snapshot exports service settings and runtime configuration in
place of desktop clash/verge/DNS files. It includes referenced global/linked/raw
profile files and saved selections, adds a schema-1 per-entry SHA-256 manifest and
excludes runtime auth/socket/lock, journals, binaries, resource caches and orphan
files. It is explicitly a service backup; upstream desktop restore is not promised.
The authenticated empty-body POST endpoint returns binary content with bounded
size/count/work, descriptor-safe reads and body-owned single admission. No caller
path or retained temporary archive is introduced. Restore/retention/schedule,
WebDAV clients and UI remain pending; upstream insecure WebDAV TLS is not copied.

Pinned zip 8.6.0 matches the source crate, with default features disabled for Stored
archives; typed-path 0.12.3 is locked transitively. futures-util moves from dev-only
to a production dependency for bounded download streaming. Source schemas and
lifecycle APIs remain Tauri-free. Tests cover ZIP/digest/raw-record preservation,
source/path bounds, links/FIFO/modes, cancellation, HTTP controls, download
admission/drop and running/stopped/restart snapshots with real Mihomo.

## Strict service backup inspection before restore

Reference: `src-tauri/src/feat/backup.rs::restore_local_backup` and
`restore_webdav_backup`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Upstream opens ZipArchive and extracts into the application directory under the
profile write lock. This increment supplies the validation phase needed before
a service-owned restore transaction; it does not copy direct extraction or
desktop/WebDAV configuration recovery.

Destinations: headless-core/src/backup.rs (bounded inspection report),
config/profile_store/sequence.rs (existing pure validation exposed for reuse),
service/src/backup_inspect.rs and management/http.rs. A bounded Stored ZIP32
preflight checks duplicate central names before zip 8.6.0 indexing can collapse
them; local records and directory offsets/names/flags/CRC/lengths must agree with
contiguous ranges, exact footer coverage and private regular-file modes. Existing
zip readers verify CRC and ring verifies manifest SHA-256; files remain borrowed
from the bounded upload rather than extracted. Service schemas, controller
boundary and existing sequence/script-source validators check configuration and
references. Uploaded JavaScript is not evaluated and Mihomo is not spawned.

The authenticated binary inspection endpoint shares export admission before body
collection, with upload timeout, cooperative worker cancellation and sanitized
errors. Reports contain counts/digests and metadata without source contents or
host paths. No dependency changes are needed. Transactional restore/rollback,
retention, schedules, WebDAV and backup UI remain pending.

## Disposable restore runtime and enhancement rehearsal

Reference: `src-tauri/src/feat/backup.rs::restore_local_backup` and
`restore_webdav_backup`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Upstream extracts directly into application data and finalizes desktop config.
This bounded increment implements candidate validation before future durable
service publication; desktop/WebDAV settings and direct extraction remain absent.

Destinations: headless-core/src/backup.rs (disposable validation report),
service/src/backup_restore.rs, backup_inspect.rs, core_manager.rs and management/
http.rs. Strict verification now also supplies borrowed manifest/entry contents
for the restore path without weakening inspection. Verified files are privately
materialized, preserving raw bytes and catalog/settings records; current Geo data
is bounded/copied safely into isolated probe data. The actor prevents core
replacement while validating both archived runtime and active regeneration.

Active regeneration reuses ProfileStore::read_generation, migrated sequence/
merge/TUN/DNS/final processing and the bounded Boa worker, following existing
actor order without double derivation. Session DNS confirmations are not imported;
the report indicates suppressed provider overrides. Scripts run only in disposable
workers and their uploaded logs/diagnostics are not added to live logs. Snapshots
and regenerated configs retain separate digests; manual runtime edits are not
silently replaced or published. Current-core Mihomo probes are cancelled/reaped
and candidate identities/content are verified afterward.

Authenticated `/api/backup/validate` shares the existing upload boundary/admission,
serializes actor core use, and cleans candidates on completion/failure/cancellation
without restore journals or live config changes. No dependency changes are needed.
Transactional publication/recovery/rollback, orphan cleanup after abrupt process
termination, retention, scheduling, WebDAV and backup UI remain pending.


## Durable service restore intent and runtime-marker recovery

Reference: `src-tauri/src/feat/backup.rs::restore_local_backup` and
`restore_webdav_backup`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
Upstream directly extracts its desktop archive then repairs Verge settings; this
increment implements original service transaction logic instead of copying that
extraction or desktop restart flow. No new upstream source is copied.

Destinations: headless-core/src/config/profile_store/restore.rs, ProfileStore and
SettingsStore transaction admission, service/src/core_manager.rs startup/actor
recovery. Existing service schema, sequence/runtime validators and persistence
helpers are reused. A private journal binds previous/candidate catalogs/settings,
allocated source digests and staged runtime identity. New immutable source names
preserve archived bytes and shared references without replacing live files.
Runtime manifest commit determines rollback versus roll-forward; conflicting or
unsafe state retains the journal. Unix no-follow/nonblocking reads require the
already locked libc 0.2.189 dependency in headless-core; Linux is verified.

This is a persistence foundation, not an online restore API. Active scripts/core
validation remain caller responsibilities; rehearsal does not issue a reusable
proof. Online runtime/DNS policy, cancellation/apply/receipt, orphan cleanup,
retention, schedules, WebDAV, backup UI and other restore targets remain pending.


## Explicit stopped-core service backup restoration

Reference: `src-tauri/src/feat/backup.rs::restore_local_backup` and
`restore_webdav_backup`, pinned commit b057bd964ccd156f68bc43a3a8ed66cf3cb1cd7b.
The service continues to replace direct desktop ZIP extraction/settings repair
with original verified candidate and journal logic. No new upstream source is
copied. Destinations: headless-core/src/backup.rs policy/receipt models,
config/runtime.rs exact-byte staging, service/src/backup_restore.rs private
publication guard and post-probe integrity, core_manager.rs actor transaction and
management/http.rs authenticated binary restore route.

Explicit archived/regenerated selection retains snapshot bytes or uses existing
migrated enhancement stages. Fresh provider DNS protection reuses service
confirmation semantics. The lifecycle actor applies only while stopped, commits
through the runtime marker and recovers precommit failure; committed cleanup is
reported without undoing publication. Admission, probes/workers, cancellation and
catalog/settings/source journal machinery are reused. No dependency changes.
Running-core restoration, orphan cleanup, retention/schedules/WebDAV/UI and other
platforms remain pending.


## Running-core service backup restoration

Reference: the same pinned upstream backup restore functions and existing
`src-tauri/src/core/core.rs` restart/reload behavior. This increment copies no new
upstream source and changes no dependencies. The original service adaptation in
`service/src/core_manager.rs` connects the verified candidate/journal transaction
to existing reload, live proxy-port readback, restart/readiness and node restoration.
`headless-core/src/backup.rs` adds live-state/restart receipt fields; the HTTP route
accepts settled running or stopped actors. Cancellation bridges privately into
core I/O and joins owned operations before rollback/reaping. Precommit failures
recover old catalog/settings/runtime and restart the old core unless the manager
is shutting down. Runtime-manifest commit remains the publication boundary.
Private probe/script diagnostics are discarded; an applied running core uses
the normal core log stream. Orphan cleanup, retention/schedules/WebDAV/UI and
other restore platforms remain pending.
