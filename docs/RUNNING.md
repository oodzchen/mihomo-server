# Running the service

The Rust entry point now runs continuously and supervises one Mihomo child.
Runtime YAML import, validation, application, commit, and recovery are also
connected, along with local subscription import, selection, and persistence.
The authenticated HTTP API and WebSocket events/realtime forwarding are connected.
A minimal browser UI now uses those interfaces and is served as built assets by
Rust. The initial Linux local-profile MVP is now verified through the
[pinned bundle and foreground launcher](DEPLOYMENT.md). Direct remote subscription
import, manual refresh and linked merge/sequence/script editing are connected; scheduling,
proxy download modes, full enhancement, advanced
features and additional platforms remain pending.

## Start

Use an existing Mihomo binary. From the repository root:

```sh
cargo run --locked --offline -p mihomo-server -- \
  --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data \
  --config ./examples/minimal.yaml
```

The example starts a loopback-only mixed proxy on port 7890 with a selector
containing DIRECT and REJECT. Its MATCH rule routes through Main, so choosing
REJECT blocks matching traffic. Mihomo's own selection cache is disabled; the
service restores choices from subscription metadata. It does not enable TUN or change the system proxy.
Adjust the mixed port if another application uses it. No subscription is
required for this initial core startup.

For a built binary, use the same arguments with `target/debug/mihomo-server`.
`--mihomo` defaults to `verge-mihomo` on PATH, `--data-dir` defaults to `./data`,
and the default bootstrap config is `<data-dir>/runtime.yaml`. On first startup,
the service reads that source, creates a managed candidate, checks it, and commits
it after the core becomes ready. It does not overwrite the source file. On later
starts, the committed revision takes precedence over `--config`; modifying the
original source does not silently change the running configuration.
`--no-start` runs it with the core stopped.
Relative explicit paths resolve from the directory where the service is launched.
The management listener defaults to `127.0.0.1:9090`; use `--listen` to change it.
Listening/authentication errors terminate the service before attempting core startup.

To import or replace a runtime configuration explicitly, use:

```sh
cargo run --locked --offline -p mihomo-server -- \
  --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data \
  --import-config ./examples/minimal.yaml
```

Import validates and saves the replacement before requesting startup. Add
`--no-start` to save a validated desired configuration with the core stopped.
Import failure preserves the previous committed revision and leaves the service
running with an error and the core stopped. The foreground CLI does not attach
to an existing service; directory ownership prevents two instances. Stop the
existing foreground service before changing startup arguments. An ordinary
restart with the same `--data-dir` restores the committed YAML without needing
the original source file.

Startup writes JSON state snapshots to stdout, including phase, owned PID,
actual running version, error, last observed exit code, recovery attempt, and
readiness generation, committed `config_revision`, and `active_profile`. Phases are `stopped`, `starting`, `running`, `stopping`,
`recovering`, `failed`, and `shutdown`. Watch consumers receive the latest state;
intermediate snapshots may be coalesced.
`selection_pending` lists groups awaiting restoration; `selection_error` reports
selection, persistence/recovery, or restore deadline errors separately from core state.

A missing binary, missing/invalid core configuration, or readiness failure is
recorded as `failed` while the Rust service keeps running. Directory setup,
ownership-lock, signal-registration, HTTP listener, and authentication failures stop the management service.
A missing or invalid committed runtime file also produces a core error and can
be repaired with `--import-config`. A malformed/unsupported commit manifest is
a storage initialization error; it is reported without silently replacing state.
Malformed profile metadata, duplicate/missing UIDs, and unsafe catalog filenames
are storage initialization errors too. Missing profile content is reported when
selected, without preventing the committed runtime from starting.
Core logs are drained from startup into a bounded 200-line tail and a broadcast
stream, with the tail available at `/api/logs` and in the browser log view.
Persistent log files are not implemented.

## Bundle resource mode

The manual commands above use an explicit external `--mihomo` binary. For a
self-contained local Linux installation, prepare the pinned release bundle and
run its launcher as described in [DEPLOYMENT.md](DEPLOYMENT.md). It supplies
`--resource-dir` and requires an absolute persistent data directory.

`--resource-dir` requires a matching-target resource manifest, bundled core,
bootstrap YAML and Web build. It conflicts with `--mihomo`. The managed core is
initialized after the data lock is acquired, at `<data-dir>/core/verge-mihomo`
or `--core-dir`. First use verifies the copied bytes against the pinned SHA-256;
subsequent starts retain the existing executable so service updates do not replace
independently upgraded cores. Web and bootstrap paths default to bundle resources;
`--web-dir` and `--config` can override those paths. Committed config still wins.
Resource initialization failures stop management startup; ordinary core/config
startup failures still leave the management surface available. Full settings and
Geo/provider seeding remain pending.

## Browser UI

Build frontend assets and the Rust binary from the repository root:

```sh
npm --prefix web ci
npm --prefix web run build
cargo build -p mihomo-server --locked
target/debug/mihomo-server \
  --mihomo /usr/bin/verge-mihomo --data-dir ./data \
  --web-dir ./web/dist --config ./examples/minimal.yaml
```

Open `http://127.0.0.1:9090`. Read `./data/management-token` as the service account
and enter it on the login page. The browser verifies it with authenticated status
before displaying management views. Tokens remain in memory, never in URLs or
browser storage; refreshing requires login. Logout cancels pending requests and
closes event/feed sockets. A failed core still leaves the page and repair commands
available. Import and select a local YAML profile before saving node selections;
a bootstrap-only standalone configuration has no profile for those records.

The five navigation paths are `/`, `/profiles`, `/config`, `/proxies` and `/logs`.
Overview shows core controls and realtime traffic/memory/connection-count samples.
Overview also shows proxy connection information: configured and
core-reported HTTP, SOCKS, mixed, redirection and transparent-proxy ports,
saved-setting versus inherited values, bind address, LAN access, mode, IPv6,
and configured DNS/TUN switches. The summary refreshes on configuration/lifecycle
changes, reconnect and every five seconds, without overwriting settings drafts.
Stopped or unavailable cores show no confirmed live ports. Port mismatches are
highlighted; browser examples use only nonzero core-reported HTTP/SOCKS ports.
These reports do not test remote-node connectivity. Remote devices must use the
service host's address and enabled LAN access; wildcard bindings are not client
addresses, and the management listener is not a proxy listener.
Settings shows current ports beside each port input and in inheritance placeholders,
with saved-setting/inherited sources, stopped-core configuration values and
listener mismatches. Displaying a current port does not turn inheritance into an
explicit saved setting or overwrite an unsaved draft. These values use the same
live readback and refresh behavior as Overview.
The authenticated `proxy_access` command returns this credential-free summary.
Configuration application now checks the core-reported built-in proxy ports
against the candidate after reload and startup. A reload mismatch triggers the
existing restart fallback; a failed candidate startup rolls back the transaction.
This catches listener bind failures even when Mihomo's configuration API returns
success. Custom inbound listeners and DNS/TUN readiness remain outside this check.
Profiles accepts local file/text uploads or a remote HTTP(S) URL. Remote downloads
preserve upstream usage metadata; import and activation are separate. Profiles
may contain `external-controller`, `external-controller-tls`,
`external-controller-unix` or `external-controller-pipe` addresses from another
application. Local and remote profile generation ignores these fields because
the service supplies its own private controller. Saved original YAML remains
unchanged; selection, refresh and original-YAML edits validate the candidate with
these fields removed. Explicit runtime edits, merges and scripts still cannot
set controller addresses.
Remote profiles also offer manual refresh with active-config validation and failure recovery. The
metadata editor saves names/descriptions and supported remote settings. Noncurrent
local/remote profiles can be deleted with an inline confirmation; the active UID
remains protected even while the core is stopped.
The “合并增强” editor reads/saves a profile's linked YAML merge; “移除增强”
regenerates from its raw subscription. Current-profile changes validate/apply,
and other profiles apply their saved enhancement when selected.
The “序列增强” editor selects rules, proxies or groups and accepts prepend/append/
delete lists. Switching its type discards unsaved text; removing an enhancement
clears only that type's link.
The “脚本增强” editor saves main(config, name) JavaScript, reads existing source
and supports detach. Failures preserve the working configuration; console output
is visible in Logs. Script execution currently requires Linux.
Configuration edits validate/apply the complete runtime mapping while retaining
the active profile. Nodes supports selection and automatic-group unfix. Logs shows
the bounded core output tail, including startup failures. Background events update
state/profile/log views; feeds reconnect across core changes, and browser transport
reconnects with a fresh authenticated socket after service interruptions. Unsaved
editor text is not overwritten by events and is discarded when leaving that view.

`--web-dir` must identify a built directory with a safe `index.html`; a missing
or invalid explicit directory prevents management startup. Static files and the
login page are public under the configured Host/Origin policy. API reads/writes
still require bearer authentication. Known navigation GET/HEAD requests accepting
HTML receive the SPA entry point; unknown pages, missing assets, traversal and
external symlinks return errors. `/api` is reserved and never receives SPA HTML.
Assets use a same-origin CSP, `Cache-Control: no-store` and `nosniff`.
Node/Vite are needed only for development/build/test, not for production serving.
Use the Rust-served build for the authenticated workflow; standalone Vite preview
has no backend proxy configured. Scheduled refresh, proxy downloads, full enhancements, advanced
pages and additional system-service/platform deployment integration remain pending.
The verified Linux bundle launcher path is documented in [DEPLOYMENT.md](DEPLOYMENT.md).

Browser verification starts the built service with temporary data and a real core:

```sh
cargo build -p mihomo-server --locked
npm --prefix web run build
# Chromium must be installed for Playwright; see web/README.md.
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo npm --prefix web test
```

The test verifies failed-bootstrap repair, token rejection, local file import and
selection, node choice, invalid/valid configuration, stop/start, logs and realtime
data, service restart and restoration, event reconnect, deep-link refresh, mobile
layout and logout. Its SIGTERM cleanup requires successful service shutdown.
Further workflows cover remote refresh, metadata edit/deletion and linked merge
save/replace/reject/restart/detach while preserving raw subscription content.

## HTTP management

The service binds HTTP before starting its core initialization task. Failed
bootstrap validation/startup leaves HTTP available for repair; `--no-start`
provides the same API with a stopped core. `--web-dir` enables the browser page;
omitting it leaves API-only operation. Unknown API routes never fall back to HTML.

First startup generates 32 random bytes as a hexadecimal bearer token at
`<data-dir>/management-token`, published atomically with mode 0600 on Unix.
The token survives service restarts. It is not printed or returned by an API.
An existing token must be a regular, private file containing 64 hexadecimal
characters. Invalid credentials/files stop initialization rather than being
silently replaced. To rotate the token, stop the service, remove that file,
and restart; all clients then need the replacement token. Keep the data directory
private to the service account.

Every HTTP command/read route requires `Authorization: Bearer <token>` and the expected Host.
Browser requests with Origin must match the configured origin exactly; requests
without Origin are permitted for CLI clients. Query parameters, cookie credentials
and cross-origin CORS access are not supported. Duplicate Host, Origin or
Authorization values are rejected. All API responses use `Cache-Control: no-store`.

For local access, the default origin/Host derive from the explicit listening
address. Accessing `localhost` when configured as `127.0.0.1` requires specifying
`--public-origin http://localhost:9090`, then using that address consistently.
Wildcard addresses and reverse proxies need an explicit public origin:

```sh
target/debug/mihomo-server \
  --mihomo /usr/bin/verge-mihomo --data-dir ./data --no-start \
  --listen 0.0.0.0:9090 --public-origin https://mihomo.example.com
```

The Rust listener serves plain HTTP. In that example a TLS reverse proxy handles
HTTPS and forwards the public Host and Origin unchanged. Forwarded headers are
not trusted to override the policy. The management port cannot be zero.

Read the token locally and query state:

```sh
read -r MIHOMO_MANAGEMENT_TOKEN < ./data/management-token
curl --fail-with-body \
  -H "Authorization: Bearer $MIHOMO_MANAGEMENT_TOKEN" \
  http://127.0.0.1:9090/api/status
```

Read routes are `GET /api/status`, `/api/logs`, `/api/profiles`, `/api/config`
and `/api/proxies`. Config returns `{"yaml":"..."}` for the committed desired
revision; with no committed config it returns an operation error. Proxies
requires a running core and uses the retained client with a ten-second timeout.
Logs returns the bounded core output tail, profiles uses the upstream schema,
and state includes core/application/restoration errors.

Send commands as JSON to `POST /api/commands`. All listed reads also have a
corresponding command. Unknown commands and unknown fields are rejected.

| Command | Additional JSON fields | Success body |
| --- | --- | --- |
| `status`, `logs`, `profiles`, `config`, `proxies` | None | Same as GET |
| `start`, `stop`, `restart` | None | Core state |
| `apply_config` | `yaml`: complete standalone YAML string; clears active profile | Core state |
| `edit_config` | `yaml`: complete replacement YAML; retains active profile | Core state |
| `apply_overlay` | `yaml`: YAML mapping string | Core state |
| `import_profile` | `name`, `yaml`: strings | Imported profile item |
| `import_remote_profile` | `url`: string; optional `name`, `options` | Imported remote profile item |
| `select_profile` | `uid`: string | Core state |
| `refresh_profile` | `uid`: remote profile string | Refreshed profile item |
| `edit_profile` | `uid`: string; `patch`: metadata fields | Updated profile item |
| `delete_profile` | `uid`: noncurrent profile string | Updated catalog |
| `profile_merge` | `uid`: local/remote base profile string | `{ "uid": merge UID or null, "yaml": raw merge or null }` |
| `set_profile_merge` | `uid`, `yaml`: required strings | Updated base profile item |
| `clear_profile_merge` | `uid`: base profile string | Updated base profile item |
| `profile_sequence` | `uid`: base profile; `kind`: rules/proxies/groups | `{ "uid": auxiliary UID or null, "yaml": raw YAML or null }` |
| `set_profile_sequence` | `uid`, `kind`, `yaml`: required strings | Updated base profile item |
| `clear_profile_sequence` | `uid`, `kind`: required strings | Updated base profile item |
| `profile_script` | `uid`: base profile string | `{ "uid": auxiliary UID or null, "source": source or null }` |
| `set_profile_script` | `uid`, `source`: required strings | Updated base profile item |
| `clear_profile_script` | `uid`: base profile string | Updated base profile item |
| `global_merge` | No fields | `{ "uid": "Merge", "yaml": raw YAML }` |
| `set_global_merge` | `yaml`: required string | Updated reserved Merge row |
| `reset_global_merge` | No fields | Updated reserved Merge row |
| `global_script` | No fields | `{ "uid": "Script", "source": raw JavaScript }` |
| `set_global_script` | `source`: required string | Updated reserved Script row |
| `reset_global_script` | No fields | Updated reserved Script row |
| `select_node` | `group`, `node`: strings | Core state |
| `unfix_node` | `group`: string | Core state |

To import a browser/local file as content, this CLI example uses `jq` to escape
the YAML safely. It does not send a server filesystem path:

```sh
jq -n --rawfile yaml ./examples/minimal.yaml \
  '{command:"import_profile", name:"Minimal", yaml:$yaml}' |
  curl --fail-with-body \
    -H "Authorization: Bearer $MIHOMO_MANAGEMENT_TOKEN" \
    -H 'Content-Type: application/json' \
    --data-binary @- http://127.0.0.1:9090/api/commands
```

Use the returned `uid` in `{"command":"select_profile","uid":"..."}`, then
`{"command":"start"}` if stopped. Import saves raw YAML without activation;
selection validates with `mihomo -t` and applies through the manager. For example:

```sh
curl --fail-with-body \
  -H "Authorization: Bearer $MIHOMO_MANAGEMENT_TOKEN" \
  -H 'Content-Type: application/json' \
  --data '{"command":"select_node","group":"Main","node":"DIRECT"}' \
  http://127.0.0.1:9090/api/commands
```

Success responses are always JSON, including lifecycle mutations. Errors use
`{"error":{"code":"...","message":"..."}}`. Missing/invalid credentials,
Host or Origin produce 401; duplicate headers/query parameters produce 400;
unknown routes/methods produce 404/405; oversized bodies produce 413; JSON
content-type/decoding errors retain Axum's 415/400/422 status; failed domain
operations produce 422 with code `operation_failed`. Once shutdown admission
closes, requests produce 503. After a failed mutation, reread status to see
whether the previous core was retained or recovery failed.

The JSON envelope is limited to 9 MiB; YAML itself is limited to 8 MiB.
Reads and mutations share authentication. Writes and config reads reuse the
serialized actor, including rollback, profile/current commits and node records.
There is no arbitrary filesystem/shell command or generic Mihomo mutator route.
Rules/providers/connections/delay HTTP command adapters remain pending.
Read-only connection data is available through
the WebSocket feeds below.

On SIGINT/SIGTERM the entry point closes command admission and signals HTTP
graceful shutdown, then cancels/reaps the managed core. It waits up to ten seconds
for admitted HTTP connections to drain; exceeding that deadline terminates the
HTTP task and reports a service error. Upgraded WebSocket tasks are explicitly
cancelled and drained within the same deadline. Listener setup failure shuts down the
manager and releases its data lock without launching a core.

HTTP verification commands (after dependencies have been downloaded):

```sh
cargo check --workspace --locked --offline
cargo test -p mihomo-server --test management --locked --offline
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo \
  cargo test -p mihomo-server --test management --locked --offline -- \
  --ignored --test-threads=1
```

The real-service test uses a dynamically selected loopback port, starts with a
missing bootstrap, repairs through HTTP, imports/selects YAML, starts the core,
persists a node, rejects invalid rules, reloads valid config, stops/restarts,
restarts the whole service with the same data directory, checks restoration,
and verifies SIGTERM leaves no owned core process. It also holds authenticated
and pending WebSockets open during shutdown and checks their closure.

## WebSocket events and realtime feeds

Open `/api/events` for management events or `/api/streams/<feed>` for one of
`traffic`, `memory`, `connections`, `connections_count`, and `logs`. The endpoint
chooses a fixed feed; it cannot execute lifecycle/configuration commands or select
arbitrary internal paths. Use the HTTP command API for mutations.

Native browser WebSocket does not support a custom Authorization header. The
upgrade checks the same Host/Origin policy as HTTP, then requires a first text
frame `{"type":"authenticate","token":"<token>"}` within five seconds. No
management data is sent and no core stream is created before authentication.
An optional Authorization header, if supplied by a non-browser client, must also
be valid; it does not replace the first frame. Query tokens and cookie authentication
remain unsupported. Do not include the credential in URLs or subprotocol names.

On success the server sends `{"type":"ready"}`. A missing, invalid, oversized,
binary, or malformed first frame ends the session without returning state; token
failures/timeout send a generic error and close with code 1008. Handshake policy
failures use HTTP errors. The service accepts at most 32 total upgraded sessions,
including pending authentication; excess upgrades receive 503. Unknown feeds
return 404. Inbound frames/messages are limited to 4 KiB.

The browser uses this same-origin authentication sequence:

```js
const scheme = location.protocol === "https:" ? "wss:" : "ws:";
const events = new WebSocket(`${scheme}//${location.host}/api/events`);
events.addEventListener("open", () => {
  events.send(JSON.stringify({ type: "authenticate", token }));
});
events.addEventListener("message", ({ data }) => {
  const event = JSON.parse(data);
  // ready precedes snapshot; replace state from snapshots after every reconnect.
  handleEvent(event);
});
// Cancel this browser subscription by closing this specific socket.
// events.close();
```

For `/api/events`, `ready` is followed by:

```json
{"type":"snapshot","status":{},"profiles":{},"logs":[]}
```

Those fields contain the same core state, upstream profile catalog and bounded
core output tail available from HTTP. Subsequent events are:

| Type | Payload |
| --- | --- |
| `status` | `data`: latest core state |
| `profiles` | `data`: latest profile catalog |
| `log` | `data`: `{stream,message}` from core stdout/stderr |
| `logs_reset` | `data`: current log tail after a lagged receiver |

Watches coalesce intermediate state updates. Log broadcasts are bounded to 200
entries; a lag resets the tail rather than claiming complete history. Subscription
and snapshot capture can overlap with concurrently produced logs, so consumers
must treat snapshots/resets as replacement data rather than append-only replay.
There is no durable event replay/sequence protocol yet. Reconnecting creates a
new authenticated socket and a fresh snapshot; clients can also reread HTTP state.
Stopped/failed-core state and profile events do not require a running core.

Each realtime socket receives `ready`, then `{"type":"core_state","data":...}`.
The service starts its internal subscription when the core is Running and emits
`{"type":"data","data":...}` with the retained Mihomo response object. Feed
models are unchanged: traffic has `up/down`, memory has `inuse/oslimit`, connections
has totals and connection records, connections_count has `count`, and logs has
`type/payload`. Core logs subscribe at DEBUG level; `log-level` still controls
what the core produces. Realtime logs and service stdout/stderr logs are separate
formats, matching their respective upstream APIs.

On core stop or readiness-generation change, the service cancels that session's
old subscription and discards its queued samples. Browser sockets remain open
and receive core state. Core stream closure/errors cause a generic `stream_error`
and a retry after one second while the core is Running; connection attempts have
a five-second timeout and can be cancelled by shutdown, core changes or browser
disconnect. A later `data` frame indicates successful recovery. Browser transport
failure itself requires the browser to reconnect and authenticate again.

Each session queues at most eight samples, each at most 1 MiB, and returns false
from the retained checked callback when its consumer is gone or overloaded.
Overflow emits `error` with code `stream_overflow` when possible, then closes the
session instead of silently skipping samples. The outgoing event limit is 16 MiB
and writes have a two-second deadline. Slow/unhealthy consumers must reconnect.
The server sends ping frames every 15 seconds and requires pong responses within
45 seconds; browsers handle these control frames automatically. After authentication,
only control frames are accepted from clients; sending command/data text closes
the socket. Events and realtime feeds are server-to-browser channels.

Each feed owns a distinct internal subscription ID. Disconnecting one browser
only cancels its ID; it cannot clear another browser's feed. Core lifecycle cleanup
retains the existing global cancellation behavior. Service shutdown closes admission,
wakes pending authentication/connection attempts, drops per-session subscriptions,
and explicitly waits for upgraded sessions as well as HTTP draining. Closing the
WebSocket is the cancellation API; no browser subscriber registry is required.

Verification:

```sh
cargo test --workspace --locked --offline
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo \
  cargo test --workspace --locked --offline -- --ignored --test-threads=1
```

Socket tests verify handshake policy, no pre-authentication data, first-frame
rejection/timeout/size limits, admission bounds and pending-session cancellation,
failed/stopped-core events, profile updates and reconnect snapshots. Real tests
verify all five retained feeds, independent cancellation, internal retry, hot
reload, core restart/generation replacement, and SIGTERM cleanup through the
actual service entry point. Windows runtime verification remains pending.

## Process and controller ownership

The service holds an exclusive OS file lock at `<data-dir>/.mihomo-server.lock`.
Another instance using that directory cannot start. The lock file may remain
after shutdown; the lock itself is released after process cleanup.

On Unix, `<data-dir>/run` must be a private directory, created with mode 0700.
The internal controller uses `<data-dir>/run/core.sock`, passed with Mihomo's
`-ext-ctl-unix` argument. Both `external-controller` and
`external-controller-tls`, `external-controller-unix`, and
`external-controller-pipe` must be absent or empty in the runtime YAML. Active
sockets are rejected; stale sockets in the private directory are removed before
starting a replacement. The supervisor does not connect to the desktop core.

Startup passes the upstream `-d`, `-f`, and platform IPC arguments, immediately
drains both output pipes, and checks `/version` up to 30 times, with a 400 ms
probe timeout and 100 ms interval. Running state is published only after a
successful API probe. Failed startup terminates and reaps the owned child.

## Control and stop

`CoreManager` exposes serialized `start`, `stop`, `restart`, `reload_config`,
`import_config`, `apply_config`, `edit_config`, `apply_overlay`, and `shutdown` operations,
status subscriptions, log subscriptions, and a log tail. Import accepts a file,
apply accepts a complete YAML mapping, and overlay merges into the committed
mapping using the extracted upstream helper. A successful apply while stopped
commits the desired configuration and leaves the core stopped. These are Rust
APIs reused by the HTTP management command layer. The CLI
exposes startup import, but no live-control connection to an existing service.

Use Ctrl+C (SIGINT) or SIGTERM to stop the foreground service on Unix. Shutdown
rejects new operations, interrupts pending startup/reload, cancels recovery and
Mihomo subscriptions, and sends SIGTERM to the owned child. It waits up to five
seconds before forcing termination, then allows five seconds to reap the child.
Output readers are joined or cancelled before directory ownership is released.
`kill_on_drop(true)` is only the abnormal-drop fallback.

Unexpected exits are observed and reaped. The default recovery budget is three
automatic starts with 1, 2, and 4 second backoff. A successful recovery does not
reset that budget, so repeated crashes cannot create an unlimited restart loop.
An explicit stop cancels recovery; a new manual start or restart resets the
budget. A failed initial start waits for an explicit retry instead of looping.

## Runtime configuration transaction

Managed files live under the explicit data directory:

```text
config/
├── state.yaml            # schema version, committed revision, pending revision
└── revisions/
    └── rev-*.yaml         # independently written candidate/committed YAML files
```

Configuration import and apply use this sequence in the lifecycle actor:

1. Parse a YAML mapping, optionally apply the upstream merge overlay, and check
   private-controller constraints. YAML input/output is limited to 8 MiB.
2. Write and sync a new immutable revision; never overwrite committed YAML.
3. Run `mihomo -t -d <data-dir> -f <candidate>` with a default five-second
   deadline. Drain both output pipes concurrently, retaining at most 64 KiB per
   stream; excessive output is rejected. Check the exit status and upstream
   stderr fatal keywords. Timeout/shutdown kills and waits for the validator.
4. Journal the pending revision while retaining the committed revision.
5. If running, prefer the retained `PUT /configs?force=true`. On reload failure,
   stop/reap the old process, launch the candidate, and confirm readiness. If
   stopped, save the validated desired configuration without starting the core.
6. Atomically replace the commit manifest after success. Each manifest write
   syncs the file and, on Unix, the directory; revision/manifest files use 0600.

Rejected YAML/core tests preserve the committed revision and running PID. An
application or commit failure attempts to restore the previous manifest and,
if previously running, restarts the previous configuration. Recovery failure is
reported explicitly and may leave the core failed; the service does not claim
disk and core rollback always succeeds. A service restart discards a pending
revision and starts the committed configuration. Normal starts/restarts also
validate the committed YAML against the selected binary before spawning it.

A transient validation process may coexist with the one running core. It belongs
to the same service and is always waited for. Revision history is currently retained;
garbage collection, UI history/rollback controls, and backup policy are pending.
Provider/Geo downloads or cache side effects are outside this YAML transaction.
Scheduled refresh/proxy download modes, scripts, full DNS/TUN enhancement, and
structured validation outcomes still need migration. Ordinary deep merge, DNS
shallow merge, and hosts replacement already use the extracted implementation.

Linux lifecycle and signals are verified. Windows Named Pipe launch and console
signal adapters and storage code are present, but are not platform-tested; Windows SCM controls,
Job Object cleanup, and Windows service packaging remain pending. Deployment
integration must also handle child cleanup when the service is killed abruptly.
No system service is installed or registered by these commands.

## Verification

```sh
cargo test --workspace --locked --offline
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo cargo test -p mihomo-server \
  --test lifecycle --locked --offline -- --ignored --test-threads=1
```

Live tests require local socket binding permissions and use private temporary
data with proxy listeners disabled. They verify directory ownership, failed
startup and retry, duplicate/concurrent commands, valid and rejected reloads,
restarts, bounded crash recovery, real service SIGTERM/SIGINT handling, committed
configuration restoration without the source, pending-candidate recovery, and
repair of a missing committed YAML. A controlled Unix-socket process verifies
reload failure, failed candidate restart, rollback, and subsequent valid apply.
The default suite also exercises shutdown during readiness with a controlled
process that ignores SIGTERM, covering the forced termination path. It checks
storage recovery and failure, validation timeout/cancellation and reaping, fatal
stderr with a successful exit code, and bounded simultaneous stdout/stderr capture.

## Local subscription import and selection

To save local subscription YAML, select it, and start the core:

```sh
cargo run --locked --offline -p mihomo-server -- \
  --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data \
  --import-profile ./examples/minimal.yaml \
  --profile-name "Local subscription"
```

The imported UID is printed to stderr, and JSON status includes `active_profile`.
Stop with Ctrl+C; an ordinary launch using the same data directory restores the
committed runtime and active UID. The original import file is no longer needed.
To switch to a saved subscription on launch:

```sh
cargo run --locked --offline -p mihomo-server -- \
  --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data \
  --select-profile '<saved-UID>'
```

`--import-profile`, `--select-profile`, and `--import-config` are mutually
exclusive. Add `--no-start` to import/select a validated desired profile while
leaving the core stopped. `--profile-name` requires `--import-profile` and defaults
to the input file stem. Repeated imports create separate entries; update/delete
and URL downloads are not implemented yet. The CLI does not attach to an existing
service; stop the foreground service before using new startup arguments.

The Rust `import_profile` operation only imports a YAML mapping; `select_profile`
performs private-controller checks, `mihomo -t`, apply, and commit. Consequently
a syntactically valid but core-invalid import remains in the catalog after a
failed selection, while the previous active UID/runtime remains committed.
The CLI invokes import then selection. An import/selection failure leaves the
management service alive with the core stopped during CLI initialization; it
does not start the old profile automatically after a requested switch fails.
`profiles()` returns the catalog snapshot and `subscribe_profiles()` watches it.
Import, selection, runtime writes, and all lifecycle operations share the same
directory lock and command queue. Node selections are restored by the same actor;
see the node-selection workflow below.

The initial flow uses raw local YAML and persisted remote YAML. Direct URL
import is now available through HTTP/browser, as described below. It does not
build the full desktop enhancement chain. Linked rules/proxies/groups sequences
and YAML merge/script are applied on selection and active refresh. Script execution
is bounded on Linux; other platforms reject it explicitly. New imports do not automatically create
the desktop auxiliary enhancement items. Full service proxy-port/DNS/TUN settings
are pending, so supply required listener and routing settings in the YAML;
the example provides a port-7890 DIRECT/REJECT configuration.

## Remote subscription import

In the browser's Profiles page, enter an HTTP(S) subscription URL and optional
name, optionally enable “通过托管内核代理下载”, then choose “下载并导入”.
The server downloads and saves the raw YAML; select its UID separately to validate/apply. Import does not switch the
current profile or start the core. The list distinguishes remote profiles and
shows reported usage. The manual refresh button updates an existing remote profile
without changing its UID; the workflow is described below.

`POST /api/commands` accepts this authenticated command:

```json
{
  "command": "import_remote_profile",
  "url": "https://provider.example/subscription?token=YOUR_SUBSCRIPTION_TOKEN",
  "name": "Optional display name",
  "options": {
    "self_proxy": false,
    "with_proxy": false,
    "timeout_seconds": 20,
    "user_agent": "clash-verge/v0.1.0",
    "update_interval": 120,
    "allow_auto_update": false
  }
}
```

`name` and `options` may be omitted. With no name, Content-Disposition filename*
(percent-decoded) or filename takes precedence over the URL basename; an empty
fallback becomes Remote File. The timeout defaults to the upstream implementation's
20 seconds, accepts 1..120 seconds, and covers request/body transfer. User agents
are limited to 1 KiB, URLs to 8 KiB and profile names to 256 bytes. Downloads reject
non-success status, invalid UTF-8/YAML, oversized responses and mappings without
`proxies` or `proxy-providers`. An empty proxies list remains valid import content;
core validation happens on activation.

Direct and managed-core transport disable implicit environment proxies; all modes
follow at most ten redirects and
uses verified platform TLS with one static Mozilla/WebPKI root retry for TLS trust errors (details below). No service bearer token is sent to providers.
Request/body errors omit subscription URLs. Authenticated profile queries expose
stored subscription metadata, including its URL, using the existing private file
and HTTP boundaries. `self_proxy: true` selects the managed core's actual Mixed
listener, or its HTTP listener when Mixed is disabled. The core must be running;
listener ports must match the committed runtime snapshot. A stopped core, missing
HTTP-compatible ingress or incompatible bind address returns an error without
falling back to direct access. Direct downloads can work while the core is stopped.
`with_proxy: true` enables service system proxy discovery when self_proxy is false.
Linux uses the service process environment; browser/desktop proxy settings are not
consulted. `danger_accept_invalid_certs: true` explicitly disables certificate and hostname verification for that subscription only; the default remains verified. Linked-enhancement download options remain unsupported.

Usage accepts `subscription-userinfo` and storage-provider prefixes ending in a
hyphen, preserving upload/download/total/expire fields. Valid HTTP(S) profile home
URLs are normalized. An explicit update_interval overrides the provider's
profile-update-interval header, which converts hours to minutes; overflow is ignored.
allow_auto_update defaults to true as upstream metadata, but **no automatic refresh
runs yet**. Remote items use R-prefixed UIDs and upstream profiles.yaml layouts.
BOM is removed before raw content is saved; no formatter rewrites it and no desktop
auxiliary enhancement items are automatically created.

At most four network imports/refreshes run per manager, with writes serialized by its actor.
Declared and streamed bodies are limited to 8 MiB. Network futures do not occupy
the actor, so core control/supervision remains available during slow downloads.
Service shutdown cancels downloads and waiting admission. Invalid downloads do
not write files/catalog state; catalog-save failures remove unreferenced new files
and preserve the current runtime/profile. Existing post-rename sync ambiguity
still follows the storage semantics documented below. Startup uses the committed
runtime and saved content; it does not automatically download the remote URL again.

```sh
cargo test -p headless-core --locked --offline
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo cargo test -p mihomo-server \
  --test remote_profiles --locked --offline -- --ignored --test-threads=1
```

Local fixture HTTP servers verify metadata/redirects, authentication before fetch,
size/timeout/error preservation, bounded concurrency and cancellation. A real core
verifies activation, node records and restoration without refetching. These tests
do not require contacting a user's subscription provider.

## Manual remote subscription refresh

Choose “刷新订阅” on an existing remote profile, or send the authenticated command:

```json
{ "command": "refresh_profile", "uid": "YOUR_REMOTE_PROFILE_UID" }
```

Only `uid` is accepted; local/unknown profiles fail before any provider request.
The request reuses saved URL/user agent/timeout/update options. Manual refresh is
allowed even with allow_auto_update false. Saved self_proxy selects the managed
core; saved with_proxy enables system discovery when self_proxy is false.
The saved certificate option also applies to refresh. Linked
sequences/YAML merge/scripts are supported and applied to active updates. Refresh
uses the same direct, system or managed-proxy transport,
shared four-download admission, body limits and shutdown cancellation as import.
The automatic scheduler starts with the recovered catalog; see Scheduled subscription updates below.

A successful update keeps UID/name/description/URL and current node records, and
replaces usage/home/timestamp metadata. Missing usage/home headers remove those
old reported values. Upstream option merge semantics are retained. Provider title
headers do not rename the saved profile. Updates to a noncurrent profile only
change its saved content/catalog; the running core and current profile remain as
before. Refreshing the current profile regenerates from downloaded YAML and thus
replaces runtime-only editor/overlay changes. It performs YAML and `mihomo -t`
validation, reloads or restarts with the candidate if running, and commits through
the lifecycle actor. A stopped current profile stays stopped. Valid node records
restore after application; unavailable records follow existing reconciliation.

Each refresh uses an immutable profiles/refresh-*.yaml candidate and a private
profile-refresh.yaml journal. Failed active validation does not replace the raw
content or catalog. Application/catalog failures attempt to restore old content,
metadata, runtime and the working core; recovery failures are included in errors.
On service restart, an active journal follows the committed runtime revision and
an inactive one follows the catalog rename. Pending runtime candidates are never
promoted. Successful catalog/runtime commit can precede a failed directory sync;
the durable outcome follows the same rename semantics as the underlying stores.
Cleanup failures remain visible and recovery is retried before another command.
Retained raw and runtime revisions still need garbage collection.

The active UID is determined at commit time. Concurrent downloads check their
source file/URL/options; an older completion is rejected if another refresh already
committed. Node records are read from the actor's current catalog at commit, so a
node choice made while downloading is preserved. Other core operations remain
available during download. Refresh never starts a stopped core or refetches at
startup. Metadata editing/deletion is described below;
complete enhancement and resource rollback remain separate pending work.

The tests above additionally verify active/noncurrent/stopped updates, authentication
before refresh, invalid update preservation, post-reload catalog write failures,
reload/restart failure recovery, stale responses, node changes during download,
interrupted journal outcomes and shutdown cancellation. The release-bundle browser
tests verify actual button actions, usage updates and restart restoration.

## Profile metadata editing and deletion

Select “编辑” on a local/remote profile. Save a title/description, and for remote
items optionally change URL, user agent, timeout, download mode, update interval or auto-update
metadata. This saves catalog metadata only: UID, raw content, usage/home/download
timestamp and node records remain unchanged; no network or core reload happens.
Changed URL/options take effect on the next explicit refresh. The previous usage
metadata continues to describe the last downloaded content until then.

Authenticated commands use these strict shapes:

```json
{
  "command": "edit_profile",
  "uid": "YOUR_PROFILE_UID",
  "patch": {
    "name": "New title",
    "desc": "Optional description",
    "url": "https://provider.example/new-subscription",
    "options": {
      "self_proxy": true,
      "with_proxy": false,
      "user_agent": "clash-verge/v0.1.0",
      "timeout_seconds": 20,
      "update_interval": 120,
      "allow_auto_update": false
    }
  }
}
```

Each patch field is optional; omitted/null fields retain their values. An empty
description clears it. Titles must be nonblank and at most 256 UTF-8 bytes,
descriptions at most 4 KiB, URLs valid HTTP(S) at most 8 KiB, user agents at most
1 KiB without control characters and timeouts 1..120 seconds. update_interval is
an unsigned count of minutes; zero disables automatic scheduling.
URL/options are remote-only. Supported option fields merge into saved options;
unsupported enhancement fields and UID/type/file/selected/
extra/updated changes are rejected. The dedicated raw subscription editor edits
the source; the configuration editor edits the separate runtime YAML.

```json
{ "command": "delete_profile", "uid": "YOUR_NONCURRENT_PROFILE_UID" }
```

The browser offers inline confirmation and cancellation. API callers submit the
single explicit delete command; there is no force override. The actor rejects
canonical active UID deletion even if stopped, and also protects the compatibility
current mirror. Select another subscription before deleting the previous one.
Noncurrent local/remote deletion removes its catalog entry, node records, current
raw file and exclusive linked merge/script/rules/proxies/groups rows and files.
It also removes that UID's saved DNS preference and session confirmation, without
changing active UID/runtime/PID or runtime settings. Shared auxiliary rows, all
reserved defaults and files referenced by surviving rows remain. Missing files
and missing auxiliary rows are tolerated; unsafe filenames, symlinks/directories,
wrong-type/nested auxiliaries and incoming enhancement links to the base are
rejected. Unlinked orphan files and older immutable revisions are not collected.

Deletion stages a private profile-delete.yaml journal (version 2 with auxiliary
UID/file pairs; version 1 plans still recover). One catalog rename commits all row
removals. Atomic settings cleanup then removes the deleted DNS preference before
unreferenced current raw/auxiliary files are removed and directories synced.
Catalog save failures preserve referenced content. An interruption before catalog
commit leaves the UID/file; recovery clears the plan. An interruption after commit
leaves the UID absent and recovery finishes cleanup. Cleanup failure is observable
as a committed deletion with pending cleanup; the journal is retained for startup
or the next actor command. A failed settings cleanup retains the journal and
files, and later commands retry recovery before proceeding. Startup prunes old
preferences whose UID no longer denotes a local/remote profile, preserving runtime
settings and preferences for existing profiles. As with other stores, a post-rename directory-sync error
can occur after the catalog logically committed; read the returned/current catalog
rather than assuming every error means no change. Old unreferenced refresh and
runtime revisions remain pending garbage collection.

All mutations and recovery run under the owned data lock in the actor. An old
refresh cannot recreate a deleted UID; URL/option changes reject its stale source.
Name/description edits during download are preserved from the current catalog at
refresh commit. The HTTP/socket tests and release-bundle browser workflow verify
these behaviors and retained metadata/removal across a service restart.

## Linked YAML merge enhancements

In the browser, choose “合并增强” on a local/remote profile, edit a YAML mapping
and save. Existing comments/content are returned unchanged to the editor. The
authenticated command API uses these explicit shapes:

```json
{ "command": "profile_merge", "uid": "BASE_PROFILE_UID" }
```

```json
{ "command": "set_profile_merge", "uid": "BASE_PROFILE_UID", "yaml": "mode: rule\n" }
```

```json
{ "command": "clear_profile_merge", "uid": "BASE_PROFILE_UID" }
```

Read returns `{ "uid": null, "yaml": null }` when no link exists. Save requires
a string mapping of at most 8 MiB; null/missing YAML never clears an enhancement.
Unknown fields, unsafe file paths and non-merge/nested links are rejected. Writes
return the updated base item. Catalog auxiliary items have upstream lowercase `m`
UIDs, type `merge`, `<UID>.yaml` content and an updated timestamp, linked through
the base item's `option.merge`. Base UID/raw file/metadata/node records are preserved.
The API returns the complete catalog, while browser cards count base profiles only.

Generation reuses upstream `use_merge`: overlay top-level keys normalize to
lowercase, ordinary mappings recursively merge, arrays replace, DNS fields shallowly
merge and hosts replace. Private controller fields must remain absent/empty. Supply
ports/routing/DNS/TUN in raw or merge YAML; dedicated service settings and the full
authoritative settings chain is pending. Sequences execute before global and
profile merges, which can replace their resulting arrays. Reserved global defaults
are initialized and applied; see the global enhancement section below.

For the active UID, save/clear validates a generated runtime with `mihomo -t`
before committing the link, applies it through reload/restart fallback, and restores
the old catalog/runtime/core on failure when possible. The active UID remains
unchanged and node restoration uses its existing records. A stopped core stays
stopped. For an inactive UID, mapping/size/controller validation precedes storage;
Mihomo validation happens when selecting it. A core-invalid inactive enhancement
can therefore be saved; failed selection preserves the working profile/runtime.
Selection and active refresh regenerate from raw plus stored sequences/merge, replacing
runtime-only edits. Changing a link during download invalidates its source snapshot.

Each edit uses immutable auxiliary content and a private `profile-merge.yaml`
journal (at most 16 MiB + 4 KiB for its two catalog snapshots). Active recovery
follows the committed runtime revision; inactive recovery follows the catalog link.
Startup and subsequent actor commands finish recovery before new changes. Failed
validation writes no new link; interrupted pending runtime revisions are discarded.
Cleanup errors are reported without silently undoing an accepted runtime commit.
Replacing/detaching retires only unreferenced ordinary auxiliary catalog rows;
shared links and the reserved global `Merge` row are kept. Old immutable files are
retained pending garbage collection. Detach all links and switch away from the
active UID before deleting a base profile; automatic cascade deletion is pending.

## Linked rules/proxies/groups sequence enhancements

Choose “序列增强” on a local/remote profile, select rules/proxies/groups, and edit
its YAML. Save applies an active change after core validation; inactive profiles
save storage only and receive core validation when selected. A stopped current
profile remains stopped. Authentication, raw/base UID preservation, node recovery
and failure handling follow the merge workflow above.

Commands use explicit required fields; kind is exactly rules, proxies or groups:

```json
{ "command": "profile_sequence", "uid": "BASE_PROFILE_UID", "kind": "rules" }
```

```json
{ "command": "set_profile_sequence", "uid": "BASE_PROFILE_UID", "kind": "rules", "yaml": "prepend: ['DOMAIN,example.test,REJECT']\nappend: []\ndelete: []\n" }
```

```json
{ "command": "clear_profile_sequence", "uid": "BASE_PROFILE_UID", "kind": "rules" }
```

Read returns null uid/yaml when no link exists. Set requires at most 8 MiB of YAML
with all three prepend/append/delete lists. Unknown fields, missing/null lists,
empty rules/deleted names and non-string rules are rejected. Proxy/group prepend/
append entries must be mappings with nonempty name/type strings, for example:

```yaml
prepend:
  - {name: Added, type: direct}
append: []
delete: [OldProxy]
```

For groups, use a mapping such as `{name: Extra, type: select, proxies: [DIRECT]}`.
Delete values are exact rule strings or proxy/group names. Rules/proxies/groups
auxiliary rows use lowercase r/p/g UIDs, matching YAML filenames and type fields,
linked via base option.rules/proxies/groups. New content is immutable; replacing
it changes the auxiliary UID, while base raw content and identity stay unchanged.
Clear is explicit; null/missing YAML does not detach. Imports do not automatically
create empty per-profile enhancement items. Reserved global defaults initialize
at service startup after recovery.

Generation follows rules → proxies → groups → global merge/script → profile merge/script. Each
sequence produces prepend + retained existing entries + append; delete filters
existing entries, not newly prepended/appended ones. Proxy deletion removes group
references, and added proxy names enter the first existing selector before group
edits run. Merge arrays can subsequently replace sequence results. The final
desktop group cleanup and authoritative settings/DNS/TUN pipeline remain pending.
Saving a syntactically valid but core-invalid inactive sequence can succeed;
failed selection preserves the working active profile/runtime.

The generalized enhancement journal retains the historical profile-merge.yaml
filename/schema version. Its strict kind field identifies the changed link;
older records without kind recover as merge. It shares catalog/runtime acceptance,
rollback and startup recovery with merge. Shared/reserved Merge/Rules/Proxies/Groups
rows survive detach; unreferenced ordinary rows are retired and old immutable
files await GC. Remote refresh keeps all links and generates from their saved
content. A link change during download rejects the old response. Detach all
enhancements and switch away from the active UID before deleting a base profile.

## Linked JavaScript enhancements (Linux)

Choose “脚本增强”, edit main(config, name) and save. Scripts run after linked
rules/proxies/groups and merge, receiving a lowercased config and the profile's
display name. Return a synchronous configuration object; arrays, null, Promise
returns and execution/serialization errors are rejected. Output top-level keys
are lowercased. Source is at most 1 MiB; unsafe/nested/wrong-type links fail.

```json
{ "command": "profile_script", "uid": "BASE_PROFILE_UID" }
```

```json
{ "command": "set_profile_script", "uid": "BASE_PROFILE_UID", "source": "function main(config, name) { console.info(name); config.mode = 'rule'; return config; }" }
```

```json
{ "command": "clear_profile_script", "uid": "BASE_PROFILE_UID" }
```

Read returns uid/source or nulls. Save requires a nonempty string; missing/null
source never clears a link. Auxiliary type script/s UID/<UID>.js source/timestamp
is linked through option.script. Base raw file, UID, metadata and node records
remain unchanged. A save executes the script before writing its link. Active
changes then validate with Mihomo -t and apply through reload/restart/rollback;
inactive changes defer core validation to selection. A stopped current core stays
stopped. Clear regenerates without that script, retaining other enhancements.
Runtime-only edits are discarded by reselect/active refresh, which regenerate from
raw and saved enhancements. Startup restores committed runtime without executing
scripts or fetching providers; failed generation leaves it usable.

console.log/info/error/debug/warn/table format their first argument as JSON. Up
to 1000 entries/1 MiB of console output enter the authenticated Logs stream, which
keeps its bounded tail. Logs captured before an evaluation failure survive; worker
timeout/crash/cancellation may have no transcript. No filesystem/network/Node host
APIs are available and async promise jobs are not run. Script failures are explicit
errors rather than silently accepting the input config as desktop fallback does.
Private-controller fields must still be absent/empty after script generation.

Evaluation uses a disposable worker in the same Rust binary, with cleared
environment. Linux limits are 512 MiB virtual address space, five CPU seconds,
no core dumps and at most five seconds wall time; Boa loops also have a
10-million-iteration limit. Config JSON is bounded to 10 MiB, IPC to 20 MiB,
stderr to 16 KiB and final YAML to 8 MiB. Shutdown/timeout kills and reaps the
worker, without starting another management listener or owning the service data.
These are process/resource limits, not a general OS security sandbox. Equivalent
limits on other platforms remain pending, so those platforms reject scripts.

Source/link publication uses the existing kind-aware profile-merge.yaml journal
and catalog/runtime recovery. Shared and reserved Script rows survive detach;
unreferenced ordinary rows retire and immutable source awaits GC. Imports do not
automatically create per-profile Script items. Global defaults, ordering and
transactional commands are connected; authoritative settings and final group
cleanup remain pending.

## Profile persistence and activation recovery

The upstream file schema is retained:

```text
profiles.yaml                 # IProfiles: current + items
profiles/<UID>.yaml            # raw imported content
profiles/refresh-*.yaml        # immutable refreshed content
profiles/m*.yaml               # immutable linked YAML merge content
profiles/r*.yaml, p*.yaml, g*.yaml # immutable linked sequence content
profiles/s*.js                 # immutable linked script source
profile-refresh.yaml          # pending refresh journal, removed after completion
profile-merge.yaml             # shared pending merge/sequence/script transaction
profile-delete.yaml           # pending raw-file cleanup, removed after completion
config/state.yaml             # canonical runtime revision + active profile UID
config/revisions/rev-*.yaml    # generated/validated runtime snapshots
```

Profile content is written and synced before an atomic catalog save. Catalog and
content files are created with mode 0600 on Unix. Import does not activate an
item by itself. The service preserves known upstream fields, including remote
metadata, quota data, options, and recorded node selections.

Activation validates/applies a runtime candidate, saves `profiles.yaml.current`
as a compatibility mirror, and then atomically commits the runtime revision and
active UID together in the runtime journal. Failure attempts to restore the old
mirror, journal, and running configuration. If interrupted before the journal
commit, startup discards the pending revision and repairs the mirror from the
committed active UID. A copied desktop catalog's `current` alone is not enough to
prove that a runtime was committed; explicitly select its UID through this service.
Older runtime manifests without profile fields remain readable and represent a
standalone runtime configuration with no active profile.

Applying an overlay retains the active UID and commits the resulting runtime
snapshot. `edit_config` replaces the entire runtime mapping, including deletions,
while retaining that UID; the browser editor uses this operation so node selections
remain associated with their profile and can restore after restart. Both use the
same validation, application, commit and rollback transaction. A full standalone
runtime apply/import clears the active UID. Reselecting a profile regenerates from
its saved raw content plus linked sequences/merge/script, discarding runtime-only edits, as manual refresh also does;
standalone overlays are not yet
persisted as independent service enhancement settings. Missing profiles can still
leave a usable committed runtime snapshot; selecting the missing item reports an
error. Interrupted imports can leave unreferenced content files; cleanup is pending.
This transaction covers the active UID and runtime YAML, with linked merge/sequence/script recovery
coordinated by the journal above. Full enhancement, resource downloads and raw/
authoritative settings integration remain pending. Metadata edits and ordinary deletion
use the separate catalog/cleanup flow above. Node operations
use their own API-confirmation and profile-metadata recovery flow described below.

Profile tests verify local import/selection, rejected candidates, catalog write
failure after reload, active UID/runtime rollback, interrupted current-mirror
recovery, actual CLI import/select, and restart restoration without the source.

## Node selection and restoration

The manager now exposes `select_node(group, node)` and `unfix_node(group)`.
These operations require a running core and a committed active profile; standalone
runtime configurations have no profile in which to persist the choice. Selectable
groups include Selector, URLTest, Fallback, and LoadBalance. The backend checks
membership in the actual group, applies through the retained Mihomo API, confirms
the resulting snapshot, and saves `PrfSelected { name, now }` in that profile.
Invalid, rejected, or unconfirmed choices are not persisted. A persistence failure
attempts to restore both the prior records and the prior runtime selection/pin;
recovery failure is reported explicitly.

To import the example and choose Main → REJECT on startup:

```sh
cargo run --locked --offline -p mihomo-server -- \
  --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data \
  --import-profile ./examples/minimal.yaml \
  --select-node Main REJECT
```

For an existing active profile, omit `--import-profile`. Repeat `--select-node`
with two arguments for each group/node pair; quote names containing spaces.
`--unfix-node '<automatic-group>'` calls Mihomo's DELETE operation and removes
that group's saved choice. It is rejected for ordinary Selector groups. Node
startup operations run after core startup and conflict with `--no-start`. The
CLI keeps running after success/failure and still requires stopping an existing
service before starting another instance with new arguments.

Starts, explicit/automatic restarts, and accepted runtime/profile applications
restore the active profile's records. Selection restoration is owned by the
lifecycle actor, with no separate task mutating the core. New manual node
operations cancel the older restoration, including other pending groups, matching
the upstream supersession rule. Stopping, restarting, changing configuration,
and shutdown also cancel the old restoration. A failed configuration validation
does not discard the old pending restoration. Recorded choices for other profiles
are retained and used when those profiles become active again.

The extracted reconciliation logic retains valid group membership, uses the last
record for duplicate groups, and confirms missing groups/nodes across snapshots.
Startup keeps unavailable records because providers may still be loading.
Successful profile/runtime application permits confirmed stale-record repair;
empty provider-backed groups retain their records and continue retrying. The
actor adds bounded retries for both startup and apply restoration. Defaults are
three seconds for the first pass, ten seconds per API operation, a thirty-second
total restoration deadline, and a one-second retry interval. The first pass may
finish earlier; remaining work continues through actor timer turns. API calls
are interruptible by shutdown, and all passes are bounded by the remaining deadline.

A restoration deadline leaves the core running, reports `selection_error`, clears
`selection_pending`, and retains startup records for a future retry/restart.
Core running/readiness state does not by itself mean every saved node is restored.
The tests disable `profile.store-selected` to prove restoration comes from this
service, verify actual CLI/service restarts, and use controlled processes to cover
provider delays, rejected/unconfirmed operations, manual supersession, deadline
expiry, and shutdown during a blocked query. Live automatic-group checks currently
cover URLTest; Fallback/LoadBalance retain the shared logic but need additional
type-specific live checks. Windows runtime verification remains pending.

Run the added suite with:

```sh
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo cargo test -p mihomo-server \
  --test node_selections --locked --offline -- --ignored --test-threads=1
```

### Global enhancement defaults

The service initializes reserved Merge/Script catalog rows after recovering
pending profile transactions. Existing global sources are preserved. The Merge
template enables `profile.store-selected`; the Script template returns its input.
Profile generation applies sequences, global merge/script, then profile merge/script.
Missing profile links reuse the reserved defaults in the profile stages too, so a
custom global script can execute twice, matching upstream. Clearing a linked
profile enhancement returns to this fallback.

Global commands and browser editors are available in the subscription page’s
separate Global enhancements panel.
Use authenticated read/set/reset commands instead of rewriting owned source files. Saved global rows from a
compatible catalog are used on selection, active refresh and enhancement edits.
Startup uses the committed runtime snapshot without executing scripts; subsequent
regeneration reports invalid sources and preserves the working runtime. Custom
scripts currently require Linux bounded worker support.


Global saves retain the reserved UID and publish a fresh immutable YAML/JS file.
With an active profile, they regenerate all connected stages, validate and apply
before committing the source pointer. Failure preserves the committed runtime and
source; stopped edits remain stopped. Reset restores the corresponding upstream
template, so it does not disable the global stage. Old immutable files await GC.

Without an active profile, edits leave a standalone runtime/core untouched. Merge
input is mapping/size/controller checked using normalized overlay keys. Custom
scripts are syntax checked in the bounded worker without executing statements or
main; main presence/return/config-dependent behavior and core validity are checked
on selection. A script can therefore be saved while no profile is active and fail
later selection. Limits remain 8 MiB merge YAML and 1 MiB JavaScript. Unknown fields,
missing/null content and content fields on reset are rejected.

Example command bodies for POST /api/commands using the existing bearer policy:

```json
{"command":"set_global_merge","yaml":"mode: direct\n"}
```

```json
{"command":"set_global_script","source":"function main(config, name) { console.info(name); return config; }"}
```

```json
{"command":"reset_global_script"}
```


### Browser global editing

Open Subscription → Global enhancements → Edit global merge or Edit global script.
The editors load saved source and use the same authenticated transactional commands
as the API. Saved global rows do not appear as base subscriptions. Save closes the
editor after acceptance; reopen it to inspect persisted content. Server errors
remain visible and rejected drafts stay in the editor; console output is available
under Logs. Cancel changes no saved content. Client UTF-8 limits are 8 MiB YAML
and 1 MiB JavaScript, with matching backend limits.

Reset global merge/script opens an inline confirmation; Continue editing dismisses
it without changing storage. Confirm reset restores the backend’s default template,
replacing saved content and the draft, while retaining the reserved UID. Empty
failed-read drafts cannot be submitted; use Retry reading, paste complete replacement
content or reset to recover. The panel remains available before importing a profile
and while the core is stopped or failed. Active saves regenerate/validate/apply;
without a selected profile scripts only syntax-check and standalone runtime stays
unchanged. The global stage and missing-link profile fallback can run a script twice.

### Service runtime settings foundation

The service initializes `<data-dir>/settings.yaml` with:

```yaml
schema_version: 1
runtime: {}
```

Use the authenticated online commands below to change settings. Stop the service
before directly editing this file. For example:

```yaml
schema_version: 1
runtime:
  mixed-port: 7897
  socks-port: 0
  port: 0
  mode: rule
  allow-lan: false
  ipv6: false
  unified-delay: true
  log-level: info
```

Supported fields are the example above plus `redir-port` (non-Windows) and
`tproxy-port` (Linux). Ports are integers 0–65535; zero disables that listener.
Mode accepts `rule`, `global`, `direct`; log-level accepts `silent`, `error`,
`warning`, `info`, `debug`. Missing/null fields inherit the source configuration.
Explicit fields enter before global/profile enhancements and win in the final
candidate even if a script changes/removes them. Discarded override warnings appear
in Logs under `settings`. Imports/runtime edits/overlays obey the same explicit
authority; raw subscription contents are not rewritten.

Offline edits are loaded at startup. Existing committed runtime is restored exactly
on restart; reapply the subscription or save/import configuration to use offline
changes. First bootstrap uses settings immediately. Online updates coordinate both
settings and runtime as described below. Invalid schema/types/values/unknown fields or an unsafe/oversized
file fail startup without replacing it; repair offline. The file limit is 64 KiB
and newly persisted files have 0600 permissions on Unix. Controller configuration,
credentials, management bind/authentication, DNS/TUN and advanced settings are not
part of this subset. Keep using existing CLI arguments for service bind/resources.

### Online settings commands

Authenticated POST `/api/commands` accepts:

```json
{"command":"settings"}
```

```json
{"command":"set_settings","runtime":{"mode":"rule","mixed-port":7897,"allow-lan":false}}
```

Both return `{"schema_version":1,"runtime":{...}}`. `set_settings` replaces the
whole runtime settings subset; include every setting you want to retain. Omitted
or null fields inherit configuration, and `{"command":"set_settings","runtime":{}}`
clears all explicit settings. Schema version cannot be supplied by the client.
Identical settings are a no-op.

For an active subscription, updates regenerate its raw source and run enhancements
with new settings, validate, apply and jointly persist the result. Without an active
subscription, they update the committed standalone configuration; removing a setting
releases authority but retains the current runtime field until manually edited.
Without a committed configuration, only settings are saved for first bootstrap.
Stopped cores remain stopped. Failures before the runtime commit preserve or recover
previous settings and runtime; rejected drafts can be corrected and retried.
An error after the logical commit (for example journal cleanup) can leave the new
settings accepted. Read `settings`, `status` and `config` after an error to confirm
the result before retrying.

`settings-transaction.yaml` is service-owned. After interrupted updates, startup and
command admission use the runtime commit record to revert or complete settings
publication. Do not edit transaction files. Conflicting/unsafe journals fail recovery
and remain available for offline diagnosis; no new Actor commands are admitted
until recovery succeeds. Status/log snapshots remain observable. Recovery checks
the actual saved settings file, not only the in-memory snapshot. Settings controls
are available in the browser as described below.

### Browser runtime settings editor

Open Settings to edit the supported service runtime subset. Port fields left blank
and Inherit selections omit explicit authority; 0 disables a listener and Disabled
persists false. Save submits the entire subset, preserving unmodified fields.
Ports must be integers 0–65535. The saved-settings summary shows the last successful
read, while the configuration page shows effective runtime values for inherited
fields. Platform listener restrictions are still checked by the backend.

Save independently reads back persisted settings before confirming the result.
Backend failures retain the draft. If the service committed but returned an error,
the readback reports that the draft is already saved. A failed verification blocks
another save; use Check saved settings to read again without replacing the draft.
Reload settings replaces a dirty draft only after confirmation and a successful
read. Set all to Inherit changes only the draft after confirmation; save to apply.
Failed initial reads and unsupported setting schemas/fields cannot be saved as
defaults. Leaving the page discards its draft; authentication expiry logs out.

The page works before any runtime is committed and while the core is stopped or
failed. With no runtime it only saves settings for bootstrap; stopped updates keep
the core stopped. Without a selected subscription, removing authority retains the
current standalone value until manually edited/imported. Service bind/authentication,
resources, upgrades and backups are outside this editor's current subset.
The supported DNS/TUN fields are available below the scalar controls.

### DNS/TUN settings through the management API

The authenticated `set_settings` command now accepts optional `dns` and `tun`
objects inside `runtime`, using the same **whole-runtime replacement** transaction.
First read `settings`, preserve the other desired fields, then submit the full
runtime object. Read it back after either success or failure. For example:

```json
{
  "command": "set_settings",
  "runtime": {
    "dns": {"nameserver": ["1.1.1.1"], "enhanced-mode": "redir-host"},
    "tun": {"enable": false, "auto-route": false, "mtu": 1500}
  }
}
```

DNS supports enable, ipv6, listen, enhanced-mode (`fake-ip`/`redir-host`),
fake-ip-range/range6, use-hosts, default-nameserver, nameserver, fallback and
fake-ip-filter. Following the upstream DNS page, **false, null, blank text and
empty lists inherit**; only true/nonempty values are restored after enhancements.
DNS false therefore does not disable a source configuration that enables DNS.
Remove an override to inherit the source; editing DNS in standalone runtime YAML
is still available when no authoritative setting controls that field.

TUN supports enable, stack (`gvisor`/`system`/`mixed`), device, auto-route,
route-exclude-address, auto-redirect (Linux only), auto-detect-interface,
dns-hijack, strict-route and mtu (1–65535). False and empty TUN lists are explicit
overrides. Missing/null fields inherit. Empty section objects do not erase source
sections. Unowned DNS/TUN fields from a subscription or enhancement survive.
Unknown nested fields and invalid types/enums are rejected. Settings remain
private and bounded to 64 KiB including nested values; with a runtime, candidate
generation and Mihomo validation precede publication. Without a runtime, semantic
core validation is deferred until bootstrap, as for the existing scalar fields.

This is the typed/authority and pure derivation subset. TUN does not change host
DNS or acquire TUN permissions. Policy settings and fallback-filter/hosts remain pending. The browser now edits
all of the typed fields above and preserves other supported runtime settings.
Unknown nested fields prevent saving until a compatible snapshot is read.

### TUN-derived DNS configuration

An explicitly saved `tun.enable` runs the upstream pure TUN stage before DNS
settings and global/profile enhancements. When enabled with fake-IP mode (or no
mode), it sets DNS enable to true, derives DNS ipv6 from top-level ipv6 and adds
missing enhanced-mode (`fake-ip`), fake-ip-range (`198.18.0.1/16`) and, if IPv6 is
enabled, fake-ip-range6 (`2001:2::0/64`). Existing ranges remain; redir-host DNS is
preserved. Disabling TUN changes tun.enable without restoring an earlier DNS
configuration. An absent/null tun.enable inherits the source and skips derivation.

DNS settings apply afterward. With a DNS object saved, the fake-IP/IPv6 setup also
repairs absent/null/blank/non-string fake-ip-range6 before manual enhancements.
Derived DNS fields are not automatically authoritative: later merges/scripts can
change them unless an explicit nonempty/true DNS setting owns them. The service
does not repeat derivation after scripts or while restoring a committed snapshot.
This order applies to active-profile generation and standalone/bootstrap inputs.

Validation tests enable TUN only in stopped candidates and execute Mihomo `-t`;
running tests disable it before starting the core. Privileged native routing and
host DNS service integration are still pending. Inspect the generated config when
combining TUN, DNS settings and manual enhancements.

### Provider DNS conflicts and confirmation

For subscriptions containing nonempty proxy-server-nameserver,
proxy-server-nameserver-policy or nameserver-policy, service DNS-page overrides
need confirmation scoped to the UID, original provider DNS content and current
service session. Digests ignore formatting/object-key order and ordinary DNS
fields, but change when the UID or provider resolver/policy content changes.
The subscription is checked before manual enhancements. TUN derivation and manual
merge/script behavior remain independent of DNS-page protection.

Read through the authenticated command endpoint:

```json
{"command": "profile_dns", "uid": "SUBSCRIPTION_UID"}
```

The response has uid, source (null or a 64-character digest), requested and
enabled. These booleans describe DNS-page preference/permission, not Mihomo's
dns.enable. Reads may inspect inactive profiles and never mutate preferences.
To enable the **active** subscription after reviewing its provider DNS, submit:

```json
{"command": "set_profile_dns", "uid": "SUBSCRIPTION_UID", "enabled": true, "confirmation": "SOURCE_DIGEST"}
```

Missing/stale/wrong-profile confirmation returns HTTP success with
`{"status":"confirmation_required","source":"CURRENT_DIGEST"}` and makes no
changes. Review and explicitly resubmit; this result is not an applied update.
A successful transaction returns `{"status":"applied","state":{...}}`. Without
provider-specific DNS, confirmation can be omitted. Save DNS settings before
enabling. Disable with enabled:false and no confirmation. If the active UID
changed, the command fails instead of changing another subscription. Read
settings/status/config after an error to reconcile a possible logical commit.

When generation encounters an unconfirmed provider conflict, it suppresses the
DNS-page values and authority, then commits enabled:false only for that profile
with the validated runtime. Automatic disable also participates in active refresh
and enhancement transactions; failed validation/publication preserves or recovers
the previous runtime, subscription pointer and preferences. A bounded settings log
reports committed automatic disable. Disabled preferences stay disabled until
explicitly enabled, even when provider DNS disappears.

settings.yaml optionally contains profile_dns entries with enabled flags.
Profiles without an entry default to whether a runtime.dns object is saved. Entries
are recorded when DNS settings or an existing preference participate in a committed
candidate, preserving the initial empty-settings MVP behavior.
Confirmations are never serialized. Whole-runtime set_settings replacements
preserve this map. Confirmations survive switching in one session, but expire on
restart or provider DNS changes. Restart restores the last committed YAML without
regeneration; expired confirmation affects the next reselect/refresh/edit, and
does not retroactively rewrite the restored snapshot. Dedicated browser controls
remain pending; these commands are the current management path. Preferences for
deleted profiles are not yet pruned.

### Browser DNS/TUN editing and conflict confirmation

In Settings, choose Use this page's settings separately for DNS and TUN. Inherit
the whole section omits it from the next full replacement; it does not delete
source configuration. String fields have a separate Inherit/Specify control so
empty saved DNS strings remain distinguishable from absent fields. List inputs
accept JSON string arrays, for example `["1.1.1.1", "https://dns.example/query"]`.
Blank inherits, `[]` saves an empty list. This preserves delimiters and escaped
newlines inside strings. DNS false/empty values inherit at generation time; TUN
false/empty lists override. MTU must be 1–65535. The saved network snapshot shows
the last successful read. All settings share validation, failed-draft retention
and independent readback; uncertain saves must be checked before resubmission.

Save DNS settings before using Current profile DNS override. The panel shows
saved preference, current generation permission and detection of dedicated
provider resolvers/policies. Enable first asks the backend; a conflict opens an
explicit question for the current UID/source. Confirm applies it; Cancel changes
nothing. No changed digest is automatically accepted. Editing a draft, changing
active UID, receiving a new runtime generation/revision, or disconnecting discards
the displayed question. Reconnection reads permission again even if the committed
revision and generation match the previous session. Check DNS state also cancels the question and independently reconciles
errors without retrying a mutation. Authentication expiry logs out and navigation
cancels owned reads.

Restart expires session confirmation, but recovery keeps the committed runtime
snapshot. The displayed generation permission therefore does not claim that an
old runtime was already rewritten; inspect actual values in Configuration. The
next select/refresh/edit generates the protected candidate. These browser controls
do not implement host DNS changes or verify privileged native TUN routing.

### Final LAN binding and group cleanup

New candidate configurations now run the upstream final pure stages after all
manual merges/scripts and restored authoritative runtime settings, immediately
before staging and Mihomo validation. This applies to bootstrap, independent YAML
edits/overlays, selection, active remote refresh, enhancement edits and settings
replacement. Raw/imported/downloaded subscription files are not rewritten. Startup
recovery still loads the committed YAML without rerunning the pipeline.

If the final allow-lan value is true and an explicit bind-address names loopback
(localhost, loopback IPv4/IPv6 including bracketed IPv6 and IPv4 shorthand), the
proxy binding becomes `*`. False/missing allow-lan, missing bindings, custom
non-loopback addresses and malformed nonstring values are preserved. This only
normalizes the Mihomo candidate, not the management HTTP listen/public-origin
settings or its private controller. Inspect Configuration for actual values.

Group `use` arrays lose unknown providers and nonstring entries. Group `proxies`
arrays lose unknown string names unless the group has a surviving valid provider,
which may provide dynamic nodes. Proxy/group/provider names and upstream built-in
policies remain valid. Ordering and duplicates are preserved. Malformed shapes
and nonstring proxy entries remain for core validation; empty groups receive no
automatic DIRECT fallback, and rules are not repaired. Final fields are sorted
in upstream order, with bulky proxy/provider/group/rule sections last. A failed
validation preserves the working configuration and transactional settings/links.

### Original subscription YAML read/edit

The Profiles page now provides Original YAML for local and remote base profiles.
It reads the exact source text, separate from effective Configuration and linked
Merge/Sequence/Script editors. Comments, BOM and line endings are retained when
saving. Name, URL, usage, update time, node records and enhancement links are
preserved. Remote refresh intentionally replaces manual content later.

Authenticated management commands:

```json
{"command":"profile_raw","uid":"PROFILE_UID"}
```

returns `{ "uid": "PROFILE_UID", "revision": "OPAQUE_REVISION", "yaml": "..." }`.
Save with the revision from that read:

```json
{"command":"set_profile_raw","uid":"PROFILE_UID","revision":"OPAQUE_REVISION","yaml":"proxies: []\nmode: direct\n"}
```

The returned object is the new source snapshot. Revision is an opaque comparison
token, never a user-selected write path. Changed revisions reject stale drafts.
Only local/remote base items may use these commands; auxiliary/global items use
their existing typed editors. Reads reject missing/unsafe/oversized source files
instead of substituting an empty draft. UID/revision bounds are 256 bytes, raw
text is bounded to 8 MiB and the HTTP request envelope has its existing size bound.

Save first parses and validates the original YAML with `mihomo -t`, ignoring its
source controller addresses as described above, including
when the profile is inactive. The private-controller boundary remains enforced.
For the active UID, saved enhancements/settings/final cleanup then generate and
validate the candidate before application. Both raw and enhanced validation must
pass. Stopped cores remain stopped. Inactive updates publish only source content,
do not run linked scripts and do not change the active runtime or its revision.
The managed Mihomo binary is therefore required for any raw save, even while
stopped; malformed raw files can still be read and repaired.

Immutable source pointers use the existing profile-refresh journal with an
optional raw-edit discriminator, coordinated with runtime and DNS preference
settings publication. Recovery follows the committed runtime revision for active
edits, catalog publication for inactive edits. A provider DNS source change can
auto-disable its preference in that same commit; failed edits preserve previous
confirmation/permission. Old immutable files and validation revisions follow the
existing retention policy; garbage collection remains pending.

The browser keeps failed drafts and independently reads back after success or
error. If the response was lost after commit, it reports the saved draft without
resubmission. Failed verification blocks Save until Check original subscription
succeeds. Version changes require explicit Reload before editing the new base;
checking a different source does not silently make an old draft eligible to
overwrite it. Dirty Reload/Close requires explicit discard. Page navigation
discards drafts and cancels owned reads; expired authentication logs out.


New service imports initialize auxiliary defaults
------------------------------------------------

Every local file/YAML or remote subscription imported through the service or CLI
now receives five owned auxiliary links when no existing link is supplied: empty
Merge, identity Script, empty Rules, Proxies and Groups sequences. The comment-only
Merge intentionally omits the reserved global store-selected setting. The ordinary
Script uses the exact upstream identity template. The editors open these saved
files immediately; explicit clear still detaches a link and uses the existing
reserved fallback. Global scripts therefore run once for a newly imported profile,
and again at the profile stage if its Script link is explicitly cleared.

Import atomically publishes the base plus missing auxiliary rows after writing all
private files. A private profile-import.yaml intent journal precedes those writes,
so startup/command recovery removes partially written, uncommitted files. Published
imports retain exact rows and content; shared/reserved reused links are verified
and preserved. Unexpected file contents or catalog conflicts retain the journal
and report a recovery error. Import does not activate the profile or change the
running core. Refresh/raw edit/metadata edit preserve the owned links.

Existing saved catalogs retain their current links, including legacy missing-link
fallback. Automatic defaults apply to new service imports. Old immutable revisions
and unlinked orphan files remain outside import cleanup.


Remote downloads through the managed proxy
-----------------------------------------

Use `options: {"self_proxy": true}` with `import_remote_profile`, or change a saved
remote profile using `edit_profile` with `patch: {"options": {"self_proxy": true}}`.
Manual refresh uses the saved mode. Missing or false preserves direct transport;
an explicit false patch turns proxy mode off. The Profiles page provides both
choices and preserves failed import drafts. Persisted upstream profiles with both
self_proxy and with_proxy enabled prefer self_proxy, matching upstream precedence.

Proxy routing is resolved after bounded download admission, from the private core
controller and committed runtime. It prefers Mixed then HTTP, supports loopback
IPv4/IPv6 bindings, and rejects custom non-loopback bindings. Proxy credentials
come from private runtime authentication, checked against the usernames the core
reports. They are sent only to the local proxy; service management credentials are
never attached. Environment proxies and NO_PROXY do not override the explicit route.
Route resolution is limited to three seconds. Existing request/body timeouts,
redirect limits, 8 MiB maximum and download concurrency admission still apply.

Core stop/restart, process replacement or committed configuration changes cancel
in-flight proxy downloads. Shutdown cancels active and queued requests. Network
work remains outside the lifecycle actor. A metadata/raw/URL change during refresh
invalidates the old result through the existing source guard; successful results
continue through the original transactional import/refresh recovery workflows.
SOCKS-only ingress remains a separate pending
increments. System proxy discovery is described below.


Remote downloads using service system proxies
-------------------------------------------

Enable `options: {"with_proxy": true}` on import, or save
`patch: {"options": {"with_proxy": true}}` with edit_profile. Refresh honors the
saved flag, including while the core is stopped or failed. The page exposes
“使用服务系统代理下载” and “订阅刷新使用服务系统代理”. A missing/false flag leaves
system discovery disabled. If both flags are true, self_proxy wins and still
requires its running managed listener. To switch from managed to system mode,
explicitly save self_proxy false and with_proxy true. Failed drafts remain intact.

Linux system discovery reads HTTP_PROXY/http_proxy, HTTPS_PROXY/https_proxy,
ALL_PROXY/all_proxy and NO_PROXY/no_proxy from the **service process environment**.
Uppercase takes precedence, including an explicitly empty value. HTTP/HTTPS values
select their protocol's route; ALL_PROXY is a fallback. NO_PROXY supports the
locked Reqwest matcher rules: comma-separated domains/subdomains, IPs, CIDRs and
`*`. Matching destinations bypass the proxy. No configured proxy also permits
direct download, matching upstream's disabled/unavailable-system-proxy behavior.
The locked Reqwest implementation disables system discovery if REQUEST_METHOD is
present (CGI protection), including empty REQUEST_METHOD.

For example, start the bundle from its directory with:

```sh
HTTP_PROXY='http://127.0.0.1:7890' \
HTTPS_PROXY='http://127.0.0.1:7890' \
NO_PROXY='127.0.0.1,localhost' \
./launch --listen 127.0.0.1:9910
```

For systemd, supply these variables in the service environment or an appropriate
private EnvironmentFile. The service never changes the machine's proxy settings.
Changing its environment requires restarting the service; metadata mode edits
apply to the next download. Proxy endpoints are deployment configuration, not
arbitrary URLs accepted from management clients, and are never exposed by the API.

HTTP and HTTPS proxy endpoints (including scheme-less host:port and credentials)
are supported. Effective uppercase/lowercase variables must be UTF-8, no control
characters and at most 8 KiB each. Nonempty endpoints must parse as HTTP(S); invalid
values, port zero and unsupported SOCKS endpoints fail with the variable name
only, before contacting a provider. This prevents malformed configuration from
silently falling back to direct transport. Selected-proxy connection/status/TLS
failures also return errors; there is no retry through direct mode. HTTP basic proxy
authentication stays on the selected proxy hop, including HTTPS CONNECT; credentials
are stripped on cross-host redirects to direct NO_PROXY destinations. Management
authentication is never attached to provider/proxy requests.

Reqwest's native Windows/macOS discovery remains available in its platform code;
those runtime paths still require platform verification. Linux does not read a
desktop gsettings session, use PAC/WPAD or invoke an external discovery process.
TLS verification defaults, download/redirect/body/concurrency bounds, cancellation,
source guards and import/refresh recovery remain shared across all modes. A system
proxy download survives unrelated managed-core stop/reload; service shutdown still
cancels active and queued requests. Socks transport and
core upgrades and backups remain pending.


## Subscription TLS verification and fallback

HTTP(S) imports accept `options: {"danger_accept_invalid_certs": false}`. To edit
an existing remote subscription use `edit_profile` with
`patch: {"options": {"danger_accept_invalid_certs": false}}`. Omitted/null options
retain saved values on metadata edits; explicit false restores verification.
Strings such as `"true"` are rejected. Refresh uses the saved option, including
after restart, and changing it during a download rejects the stale result.
The Web import and metadata editor expose separate explicit checkboxes, initially
unchecked, describing the effect on HTTPS server identity verification.

Verified downloads use the platform certificate verifier first, honoring the
service's platform trust store (including Linux SSL_CERT_FILE/SSL_CERT_DIR).
For TLS/certificate/root/revocation errors they retry once with the locked static
Mozilla roots, still verifying certificate validity and the hostname. Root fallback
does not trust self-signed certificates automatically. TLS protocol-version errors
fail with a TLS 1.2/1.3 message and do not retry or enable an older protocol.
Network, HTTP status, body size and YAML errors do not trigger a root retry.

`danger_accept_invalid_certs: true` disables both certificate-chain and hostname
verification for that subscription's download and redirects. It does not change
the management listener, core controller or other subscriptions. This mode has
one attempt, keeps TLS 1.2/1.3, and retains HTTP status/YAML validation, body limits,
redirect limits, bounded admission, stale guards and cancellation. Use an appropriate
platform CA when server identity should remain verified.

Both attempts share the existing 1..120-second total timeout (default 20 seconds),
including redirects and body reads. Managed routes and credentials are resolved
once; system discovery remains explicitly opt-in. No attempt changes the chosen
transport mode or falls back to direct access on proxy failure. URLs, URL tokens,
proxy credentials and the management bearer token are excluded from download
errors; the management token is never forwarded to origins or proxies.


## Scheduled subscription updates

Enable `allow_auto_update: true` and a positive `update_interval` (minutes) on
remote import or metadata edit. The Web metadata editor exposes both values.
An absent allow flag defaults to enabled, matching upstream. Missing/zero interval,
explicit false, non-remote rows and missing UID/URL do not register a timer. A
provider's profile-update-interval header supplies hours converted to minutes
when no explicit interval was saved. Manual refresh remains allowed when automatic
updates are disabled.

The recovered catalog is scheduled automatically on service startup. First update
is due at `updated + interval`; already overdue subscriptions run immediately,
whereas absent/zero timestamps or future timestamps wait a full interval. Success
publishes the refreshed timestamp with the existing transaction. Subsequent runs
wait a full saved interval after completion, including failed runs, so an unchanged
old timestamp cannot cause a busy retry loop. Retry deadlines are in memory;
restarting after a failure recomputes the overdue timestamp and may retry at startup.
Unrepresentably large intervals stay dormant until metadata changes; they do not
panic or turn into immediate deadlines.

Metadata/deletion/profile watches reconfigure schedules. A successful manual
refresh changes updated and resets its pending deadline. Interval changes recompute
first due time when idle; during a run they take effect for the next full interval.
Disable/delete removes idle timers but retains an in-flight UID guard until its
worker finishes, so disable/re-enable cannot launch duplicate automatic work.
Changing URL/options while downloading uses the existing stale-result guard.
Queued automatic downloads recheck their saved source and enabled policy after
admission, before contacting the provider. Automatic commands also check enabled
policy at actor admission. Manual requests retain their existing concurrency and
source-version conflict rules.

At most four automatic workers run at once; additional due UIDs remain in the
scheduler map. Workers share the same four-download semaphore with manual refresh
and imports. Network work stays outside the lifecycle actor; active updates reuse
enhancement, YAML/Mihomo validation, application, rollback, node restoration and
journal recovery. Direct/system mode can update while the core is stopped. Managed
mode retains its running-core requirement and no direct fallback; timers wait
through starting/recovering/stopping phases and wake on stable core state.

The scheduler uses the saved proxy, TLS, user-agent and timeout options unchanged.
Its bounded `scheduler` log stream reports starts, completions and failures without
provider bodies, URLs, names or credentials. Shutdown cancels and drains all owned
workers before returning; no independent timer process or service is needed.


## Stable core release query and compressed preparation

Use the existing authenticated POST /api/commands endpoint:

```json
{"command":"core_release"}
{"command":"core_release","version":"v1.19.31"}
{"command":"prepare_core_upgrade","version":"v1.19.31"}
{"command":"prepared_core_upgrade","id":"v1.19.31-<64 lowercase SHA-256 digits>"}
```

An omitted version queries the latest published stable release. Only Linux x86_64
amd64-v2 packages are supported. The response includes version, target, asset,
compressed bytes, SHA-256 and the fixed public download URL. Preparation returns
an immutable ID plus that release object; use the actual returned ID for readback.
These commands reject extra fields, arbitrary URLs/checksums/filesystem paths,
Alpha versions, duplicate assets and missing digests. The public GitHub API needs
no management token; rate-limit/status failures are reported without relaxing
validation. Discovery and preparation share one slot; concurrent requests fail
immediately. The management listener remains available during network work.

Preparation requires --resource-dir (the bundle launcher supplies it). It saves
a verified compressed package under the private persistent managed-core directory's
.upgrade-staging, with 0700 directories, 0600 package/manifest files, fsync and an
atomic publication. Readback rechecks the manifest and package hash, including
after restart; cache reuse still resolves and checks official metadata. Errors,
timeouts and shutdown cancel preparation and remove pending files. Startup removes
only recognized real pending directories, preserving completed candidates and
unknown files. Returned responses do not expose local staging paths.

Core release downloads currently use direct HTTPS with the platform trust store,
TLS 1.2/1.3 and restricted GitHub redirects. Subscription proxy settings, service
proxy environment, static-root retry and subscription certificate bypass do not
apply to these commands. Metadata is limited to 1 MiB/20 seconds; compressed
packages to 64 MiB/300 seconds. Declared length, official SHA-256 and gzip magic
are checked. This prepares compressed bytes only: decompression, executable and
configuration validation, installation, managed switching and rollback are pending.
The current core stays in place and its running/stopped state is preserved. No
automatic core upgrade is scheduled; completed-candidate cleanup is also pending.


## Validate a staged core executable

After prepare_core_upgrade, pass its returned compressed-package ID:

```json
{"command":"stage_core_upgrade","id":"<prepared package ID>"}
{"command":"staged_core_upgrade","id":"<returned stage_id>"}
```

Both commands require authenticated management access and bundle-managed resources.
There is no binary/path/config/checksum override. The service actor snapshots its
current configuration (at most 8 MiB), serializes staging with configuration and
lifecycle operations, and leaves the existing core running or stopped as it was.
Without a readable valid current configuration, staging fails. Additional upgrade
queries/preparations/staging/readbacks are rejected while staging owns admission,
including when its original HTTP caller disconnects. State/WS transport remains
available; other actor operations wait until staging finishes.

The service rechecks the compressed digest, decodes a single gzip stream, validates
CRC and EOF, rejects trailing data or extra members and bounds the executable to
128 MiB. Extraction checks shutdown and a 15-second clock between chunks. Linux
x86_64 ELF format is required; scripts/other architectures are rejected. The private
candidate runs -v and must report exactly the pinned release version, then runs -t
against the captured YAML. Each process has a five-second deadline, bounded stdout/
stderr capture, and kill/reap on timeout or shutdown. Raw probe output is not
returned in errors. It uses a disposable resource directory with copies of known
Geo files, capped at 256 MiB combined. Other provider/resource workflows are still
pending, so resource-dependent configurations can fail this isolated validation.
Production extraction is in process and does not require a system gzip program.

Success returns stage_id, prepared release metadata, executable size/SHA-256 and
configuration SHA-256/revision. Artifacts remain private (0700 executable/directory,
0600 manifest/config); readback verifies their integrity after restart. Each ID
identifies the package and YAML hashes. Repeating staging revalidates with probes,
then reuses an intact existing artifact; its original revision is historical even
if identical YAML has since been committed again. The stored proof does not imply
the current configuration/resources are still identical, and no core is installed
or activated. The next activation workflow must check them again. Failed/shutdown
work removes temporary files; completed candidates remain until future cleanup.


## Activate a verified managed core

After stage_core_upgrade, use the returned stage_id:

```json
{"command":"activate_core_upgrade","id":"<stage_id>"}
{"command":"core_installation"}
```

These authenticated commands require bundle-managed resources. Activation does not
accept paths, checksums, configuration overrides or force; selecting a staged ID
explicitly replaces it even when the version is unchanged. The stable upgrade
wrapper and Web controls below provide latest-version/no-op/force behavior.
The returned report includes upgraded, from, to, installation and current status.
core_installation returns null for an untouched bundle seed, or the last committed
receipt with stage/version/target/executable/configuration hashes and byte count.
Readback checks its installed bytes and detects independent file changes.

Activation holds the shared upgrade slot in the actor through completion even if
the HTTP caller disconnects. Lifecycle/configuration requests wait; HTTP state and
WebSocket transport remain available. Current normalized YAML must match the saved
configuration hash, otherwise stage again. The service reruns bounded extraction,
version and isolated configuration/resource validation before creating backups.
Only Linux x86_64 ordinary executables are supported; capability-bearing/set-ID
cores require the pending native privilege integration before upgrade.

A private .core-upgrade directory holds bounded hash-checked old/new copies and a
versioned pending journal. The service stops/reaps the old child, atomically renames
the new executable, starts it and verifies its controller version plus actual proxy
ports. A second check after the configured short probe interval catches immediate
exits. Running cores remain running and restore recorded nodes. A previously stopped
core briefly starts for verification, then stops again before the committed marker.
The report's to and receipt version reflect that checked runtime; stopped status
has no running PID/version. Runtime configuration, active subscription, settings
and node records are not replaced by activation. Proxy traffic can pause during
core replacement and resumes after readiness/selection restoration.

Before the commit point, failure/shutdown stops and reaps the candidate, restores
previous bytes/permissions/receipt and restarts the old core only if it was running
and the service is continuing. A failed upgrade remains an error even when rollback
restores service. After commit, metadata/cleanup errors retain the new core and are
recovered from the committed journal. An older or changed bundle seed never overwrites
an existing activated core. Initial construction recovers interrupted switches
before seeding/startup; actor admission also retries recovery. Known partial work is
cleaned without following links or deleting unknown files. Corruption/conflicting
live bytes retain the journal and fail instead of guessing or overwriting them.

Linux core and validation children terminate on emergency parent death, allowing
pending filesystem recovery after a management-process crash. Normal service
termination still gracefully stops/reaps its children. Upgrade backups preserve
ordinary file bytes/mode; privilege xattrs are not transferred. Verification covers
bounded startup readiness, not continued health indefinitely after commit. Completed
compressed and executable candidates remain cached; their garbage collection is
pending.

## Upgrade the latest stable core from the browser

Open `/core` (内核升级) in the authenticated management page. Installed version
and installation receipt are read from the managed file even while stopped.
Check updates to display official latest metadata; upgrade resolves latest again
at the time of the action. The API accepts exactly these shapes:

```json
{"command":"installed_core_version"}
{"command":"upgrade_clash_core","force":false}
{"command":"upgrade_clash_core","force":true}
```

`force` is a required boolean. The result preserves upstream's
`{"upgraded":true|false,"from":"v...","to":"v..."}` shape. When already at
the latest stable version and force is false, no package is downloaded, no file
is replaced and no core is restarted. With force true, the service installs the
latest version even if unchanged. A verified cached compressed package may be
reused; its digest and executable/configuration probes are checked again.

One upgrade admission slot spans metadata discovery, the actor version check,
download and final actor-owned staging/activation. Download remains outside the
lifecycle actor, so lifecycle/configuration work can continue until switching
starts. Resolved release metadata/hash remain pinned if latest changes during
download. A second installed-version check and fresh configuration snapshot run
after download. Once admitted to the actor, switching continues after browser
disconnect; queued switching can be skipped if its caller has already gone away.
Shutdown cancels network work and uses the existing switch rollback/reaping path.

The page disables duplicate actions, confirms force reinstall, clears obsolete
results before retry, refreshes installed information after success or failure,
and rereads it after reconnection/navigation. Upgrade failure remains visible;
refresh installation information and retry after addressing the reported error.
An unmanaged `--mihomo` service shows that online upgrading requires a bundle.

This increment supports stable Linux x86_64 ordinary managed executables, including
repair of unreadable/empty/nonexecutable existing files. Alpha and other platforms
remain pending. The browser does not accept package URLs, paths, hashes or version
overrides for the latest-stable wrapper.

## Core download routing and TLS verification

Latest/pinned discovery tries the running managed HTTP/Mixed listener, service
system proxy policy, then direct. Managed routing reads actual ports and committed
authentication; unavailable/stale routes are skipped. System policy validates
environment/native discovery with NO_PROXY rules; NO_PROXY=* and CGI REQUEST_METHOD
bypass system discovery. Native Windows/macOS validation and SOCKS/PAC are pending.

Each metadata policy has one 20-second budget including TLS-root retry; managed
resolution adds at most three seconds. The successful policy is retained for the
pinned package with one 300-second budget. Package failure returns an error without
switching policies. Managed lifecycle/configuration changes cancel downloads and
require a fresh request; shutdown cancels the whole chain. Network stays outside
the actor; activation retains its existing serialization/rollback rules.

Platform verification is preferred; certificate-related failures retry with locked
Mozilla roots. Both modes validate certificates/hostnames, require TLS 1.2/1.3 and
use the same policy. Legacy TLS does not trigger root retry. No invalid-certificate
or caller proxy/URL option is accepted for core upgrades. Owned partial files are
cleaned before retry; completed candidates remain pending garbage collection.
Cached records contain metadata/hashes, not proxy credentials or policy choices.
The core-upgrade log records only managed/system/direct for successful discovery.

## Repair an unreadable or empty managed core

The management service and `/core` page remain available when an existing safe
managed core is empty or lacks owner read/execute permissions. The file is kept;
startup does not silently reseed it. The installed-version query returns `unknown`
when permissions/size or a bounded executable probe prevent version readback.
The page shows `未知（需要修复）`; choose `升级至最新稳定版` to repair. An old
installation receipt may fail verification against the broken file; that error
remains visible and does not disable repair when the file itself is admitted.
Default upgrades never skip an unknown version. Success reports `from: "unknown"`
and writes a verified receipt. Refresh/reconnect rereads the actual installed file.

A repair retains the old inode through a hard link in the private switch directory,
without reading or changing its permissions. Failed readiness/version/port checks,
shutdown or an uncommitted process crash restore that inode and its previous receipt.
An interrupted rollback can be resumed. A committed switch retains the verified
replacement. Existing schema-1 normal-upgrade journals remain compatible; repair
journals use schema 2 with file identity checks. Files remain limited to 128 MiB;
links, shared files, foreign ownership, group/other-write or privilege permissions,
capability-bearing upgrades and unsafe/malformed receipts are rejected. Unexpected
live/backup changes preserve the transaction for recovery instead of overwriting.

A stopped/failed core stays stopped after successful repair: explicitly start it
and verify proxy traffic. The existing profiles, runtime YAML and saved node
selections remain authoritative. Failed repair leaves a broken original broken;
management remains available to retry with a valid candidate. Alpha, other targets
and native privilege integration remain pending.
