# Architecture

This document describes the current product architecture and its important
invariants. It is not a changelog or a per-task verification log. Operational
instructions belong in [RUNNING.md](RUNNING.md), packaging and installation in
[DEPLOYMENT.md](DEPLOYMENT.md), and copied-code provenance in
[UPSTREAM.md](UPSTREAM.md).

## Current state

The active Linux scope is implemented and verified on x86_64. The product is a
single systemd service entry that serves the management application and owns one
Mihomo child process. Node and subscription data, configuration, core upgrades,
Geo resources, rules/providers, delay tests, localization, backup/restore, and
multi-user installation are connected end to end.

Production needs neither a desktop environment nor a Node/Vite process. “Single
service” still means two runtime processes: the Rust supervisor and its managed
Mihomo child.

```text
Browser
  │ authenticated HTTP + WebSocket
  ▼
Rust management service
  ├─ Web assets and management API
  ├─ profile, settings, resource and backup stores
  ├─ serialized configuration/lifecycle actor
  ├─ scheduler, downloads and bounded script worker
  └─ private Mihomo controller client
                  │
                  ▼
          managed Mihomo child
          ├─ proxy listeners
          └─ optional system-wide TUN
```

The management listener is established before core startup. It remains usable
while Mihomo is stopped, starting, recovering, or failed, so configuration and
core problems can be repaired through the same interface.

## Component boundaries

| Component | Responsibility |
| --- | --- |
| `crates/headless-core` | Tauri-independent domain logic: profile/catalog schema, settings authority, enhancement generation, runtime revisions, resource paths, backup models and recovery plans. |
| `crates/mihomo-client` | Typed Mihomo control API over Unix socket or explicit loopback HTTP, including realtime streams. |
| `crates/management-client` | Client side of the management API shared by the command line and the desktop client (a separate repository that pins this crate by release tag): locating this user's instance from systemd, authenticated commands with typed errors, and pure readings of proxy groups, subscriptions, mode and TUN. |
| `crates/clash-verge-*` | Small reusable upstream components: drafts, admission limiting, locale resources and Unix signal handling. |
| `service` | Axum management surface, authentication, durable stores, core actor, downloads, resource/core updates, backup/restore, multi-user isolation and shutdown. The same executable is the `mihomo-server` command-line client (`service/src/cli`): `serve` (or a leading service option) runs the service, any other command is a client. |
| `web` | React browser client. It talks only to the Rust service, keeps session state local and shares the interface language through the instance's preferences. |
| `deploy` and `scripts` | Pinned bundle creation, installation, systemd integration, per-user helper and lifecycle checks. |

`headless-core` must remain independent of Axum, React, systemd and process
supervision. The service owns I/O and orchestration. The browser never connects
to Mihomo’s private controller directly.

## Configuration authority and application

The persisted subscription YAML remains the source document. Service-owned
settings and enhancement records are separate authorities layered onto it;
generated runtime YAML is an output and is never treated as the editable source.

```text
subscription source + selected profile
        │
        ├─ global/profile merge, sequence and script enhancements
        ├─ authoritative service, DNS, TUN, Geo and resource settings
        └─ multi-user port/TUN isolation
        ▼
candidate runtime revision
        ├─ schema, path and resource checks
        └─ mihomo -t validation
        ▼
hot reload, or supervised restart when required
        ▼
runtime/readback verification → atomic commit
                         failure → previous revision and process recovery
```

One actor serializes lifecycle changes and all operations that can change the
effective configuration. Durable journals distinguish prepared, applied and
committed state. On interruption, startup resolves journals before serving new
mutations and either completes a known commit or restores the last committed
revision. Catalog pointers, settings, active profile and runtime manifest are
updated as one coordinated operation rather than assumed to be one filesystem
write.

Important invariants:

- A candidate is never committed before YAML/resource checks and `mihomo -t`
  succeed.
- Failed reload, restart, profile switch, settings edit, restore, core upgrade
  or live Geo replacement preserves or recovers the last working state.
- Core state, generated configuration and persisted authority are independently
  readable; API receipts do not substitute for readback.
- Node selections are stored per profile and reconciled after starts and
  accepted configuration changes.
- Source files and protected service resources are accessed with confined,
  no-follow filesystem operations; provider caches use service-owned identities.
- Script enhancements run in a separate, bounded Linux worker process.

## Core lifecycle

The core actor is the sole owner of the Mihomo child. Its externally visible
phases are `stopped`, `starting`, `running`, `stopping`, `recovering`, `failed`
and `shutdown`. It drains child output into a bounded log stream, probes
readiness, reaps every child, and limits automatic recovery attempts.
Proxy listeners can settle after the core API becomes ready. The actor polls
listener/TUN readback for up to four seconds before accepting startup or a reload;
persistent port conflicts still fail and preserve the last committed runtime.

Valid configuration changes prefer Mihomo hot reload. The actor falls back to a
restart where required and verifies the resulting process, controller and proxy
listeners before committing. Unix SIGINT, SIGTERM and SIGHUP enter the same
coordinated shutdown path: HTTP/WebSocket work drains, scheduled/background
operations stop, and the child is terminated and reaped.

Stable and Alpha core upgrades use a managed per-user core directory. Downloads,
decompression, executable/version checks and configuration probes occur before
activation. Activation is journaled and rolls back to the previous executable
when health verification fails. A newer user-managed core is not overwritten by
the bootstrap core during ordinary service startup.

Node tests (`probe_proxies`, the Web proxy page) do not use the running core's
delay API, which reuses pooled node sessions and, with `unified-delay`, reports
only a request over an open connection. The service starts a short-lived,
unprivileged Mihomo under `<data>/run/` with the committed revision's nodes,
providers (read from their caches), node resolvers and dialer options, no
listeners, TUN, rules or persisted state, bound to the default-route interface
so that it bypasses any TUN. Through it each node gets one cold HTTPS request
(node DNS, handshake, remote dial and target TLS) and one over the session that
opened, 16 nodes at a time; a group is tested through its current node. One
probe runs at a time and its process and directory are removed afterwards.

Unlock tests (`unlock_services`, `unlock_test`, the Web unlock page) test one
node at a time, isolated like node tests: neither the active rules (which may
send a service to another group or DIRECT), nor the running core's TUN or
listeners, nor the host's system proxy affect them. The page tests the node it
names, by default the current exit: the final `MATCH` rule's target (or the
mode's), followed through each group's current choice. For each node the
service starts, and reuses while tests continue, an isolated core with the same
nodes, providers, resolvers and dialer options, bound to the default-route
interface, in global mode with `GLOBAL` set to that node and a mixed listener
on a random loopback port that requires a random password (loopback is shared
with every local user). Checks are HTTP clients of that listener. Services that
answer QUIC clients differently (DMM TV refuses over HTTP/3 what it accepts over
HTTP/2, and browsers switch to HTTP/3 after the first response advertises it)
are also asked over HTTP/3: `service/src/http3.rs` carries QUIC through the
same listener's authenticated SOCKS5 UDP relay, racing a few handshakes since
some nodes lose many; a node without UDP falls back to the HTTP/2 answer, as a
browser would. At most two
such cores run at once; one unused for a minute is stopped and its directory
removed. The catalog and the detection logic (IP location databases, Google's
and other platforms' placement, streaming, AI and store checks) live in
`service/src/unlock.rs`. Each command runs one service's check under a
whole-check timeout and returns a verdict, the region the service reports, a
reason code and untranslated details. The page runs a bounded number at once
and keeps the results across navigation until the node they were taken with
changes: a different active subscription, committed revision, chosen node or,
when following the current exit, node selection clears them and discards tests
still in flight. AI services that look the same to anonymous clients everywhere
are judged from the region they see against the regions they exclude.

## Management and browser boundary

Axum serves the built SPA, authenticated command endpoints, binary backup
endpoints, and WebSocket state/log/metric feeds. Unknown API or asset routes do
not fall through to the SPA. A private management token is created in the data
directory; a wildcard listener also requires an explicit public origin.

The management command allowlist covers:

- service/core lifecycle, state, logs, core upgrades and program upgrades;
- local and remote profiles, raw YAML, metadata and scheduled refresh;
- global and profile merge/sequence/script enhancements;
- typed settings, DNS/TUN policy and generated/live readback;
- proxies, persistent selections, rules, providers and delay tests;
- Geo/provider inventory, validation and controlled updates;
- backup export, inspection, restore and private retained archives.

The Web UI controls the core (Core page) and the service itself (Service page)
separately. The service reads its own systemd unit from its cgroup, accepting it
only when that unit's main process is this one or an ancestor (an
`INVOCATION_ID` inherited from a terminal does not count). Stop and restart are
queued with `systemctl --no-block` after the response is sent, so systemd records
the intent. A service started directly can only be stopped, through the same
graceful path as SIGTERM. Upgrades never run as the user: `upgrade_service`
starts the installer's root oneshot unit `mihomo-server-update.service`, which
takes no input and runs `mihomo-server update`, so it installs only the latest
release of the built-in repository, checksum-verified by the installer. Polkit
lets `mihomo-tun` members start that unit without a password. The installer
then restarts every running instance, so the page reconnects to the new
release. The page polls `service_info` for the unit's state and the tail of its
world-readable `/var/lib/mihomo-server/update.log`. The latest tag is read
through the same managed/system/direct routes as core releases, but the unit
itself downloads directly as root.

Every open page (the desktop client's dashboard window included, which has no
refresh of its own) compares, on each WebSocket reconnect, the hashed script and
stylesheet names in a fresh `/` with its own, and reloads when they differ, so a
service that came back upgraded never runs an old page against a new API. The
reload waits while the settings editor has unsaved edits. An upgrade the
Service page was following is kept in `sessionStorage` and resumed after the
reload, which also reports a result that settled just before it.

The `mihomo-server` command is a second client of the same authenticated command
API, with no private path into the service. It locates the invoking user's
instance from the systemd unit's main process (listener, public origin and data
directory from its command line), so it sends the Host the service authorizes and
reads the token the service wrote. Service lifecycle is delegated to the release's
`mihomo-server-user` helper and program updates/uninstall to the installer, so
systemd and root actions keep a single implementation.

The desktop client, [mihomo-server-desktop](https://github.com/oodzchen/mihomo-server-desktop),
is a third client of the same API, kept in its own repository and installed,
versioned and released independently of the service. It depends on
`crates/management-client` through a git dependency pinned to a service release
tag, so a change to that crate reaches the client only when the client moves its
pin; keep the crate's public API compatible across releases. Its contract with
this repository: the management window loads the service origin directly and
logs in through the URL fragment; it subscribes only to
`/api/streams/preferences`, never `/api/events`; the Web settings page shows the
client's own start-at-login switch only when the page runs inside the client
(`__MIHOMO_DESKTOP_VERSION__`), and calls the client's `client_autostart` /
`set_client_autostart` commands through Tauri IPC; the service lifecycle goes
through the release's `mihomo-server-user` helper and installation through the
published `install.sh` (`MIHOMO_INSTALL_ELEVATE=pkexec`,
`MIHOMO_INSTALL_PROGRESS=1`). Its internal design is described in that
repository's `docs/ARCHITECTURE.md`.

Release builds of the service report the release tag as their version: CI sets
`MIHOMO_SERVER_VERSION` to the tag without its `v` when building, checks
`--version` against it, and local builds fall back to the Cargo package version.
Settings reads the service version from `service_info`,
as the Service page does, and refreshes on reconnection and core generation
changes. Core and service reads complete independently; the desktop version
comes from the host client's immutable build value, not the service.

Browser operations use independent readback after mutations. Realtime feeds can
disconnect and resubscribe without becoming configuration authority.

The interface language is a per-instance presentation preference, kept apart
from the core's settings in `<data-dir>/preferences.json` (owner-only, replaced
atomically) and never applied to the core. `preferences` reads it and
`set_language` changes or clears it (`null`: each client uses its own default).
Each change is persisted, then pushed as a `preferences` event on `/api/events`
(whose snapshot also carries it) and on the lightweight
`/api/streams/preferences` feed. The Web UI adopts the pushed language after
login and writes changes made in its settings; the login page keeps a browser
copy until then. The desktop client reads the preference when it connects, so
its tray opens in the instance's language, then follows the feed, and keeps a
copy in `$XDG_CONFIG_HOME/mihomo-server-desktop/settings.json` for when no
instance runs. Its status page changes the preference with `set_language`
while connected; otherwise the choice is saved there and becomes the
instance's preference on the next connection if the instance has none.
Without either it uses the system locale. An unreadable preferences file is ignored
(and replaced on the next change) rather than stopping the service.

Update checks are remembered the same way in `<data-dir>/update-checks.json`:
`core_release`/`alpha_core_release` (latest) and `service_release` record the
found version with the version installed at that moment, and `update_checks`
reads them. The `/core` and `/service` pages show one button, `检查更新` /
"Check for updates", which becomes `更新至 <version>` while a recorded check found
a version other than the installed one, so the offer survives reloads and
service restarts. A record whose installed version no longer matches says
nothing about updates; the next check replaces it. Failing to save a record
never fails the check itself.

## Resources, downloads and backups

Geo files and provider caches live below the managed data/resource roots. Their
paths are normalized and checked against collisions, links and protected files.
Geo updates are downloaded to private candidates, hash-checked, parsed where a
strict parser exists, tested with Mihomo, then published through the actor. A
running-core replacement uses a durable rollback journal.

Remote subscription, core and Geo downloads are bounded and cancellable. They
support direct, managed-proxy and system-proxy routing as appropriate, explicit
TLS roots/options, and retry rules controlled by the service. Secrets and source
URLs are not returned in public resource reports.

Backup archives carry a manifest and content hashes. Inspection is read-only;
validation rehearses the restored catalog/settings/runtime in an isolated
candidate; restore is an actor-owned transaction with rollback. Retained local
archives are private and bounded. Automatic retention policies, scheduled
backups, change-triggered backups, WebDAV and a backup Web UI are deferred.

## Multi-user installation and TUN

The default Linux installation shares immutable program files under
`/opt/mihomo-server` while each account owns an independent systemd user service,
data directory, token, managed core, profile catalog and proxy ports. A root-run
installation may install only the shared program; users initialize and control
their own instances through `mihomo-server` (linked into `/usr/local/bin` with its
manual and completions) or the underlying `mihomo-server-user` helper.

Slots provide deterministic non-overlapping management, proxy and DNS ports.
Port rewriting is applied as a final generated-configuration overlay, leaving the
subscription source unchanged. Explicit user settings remain authoritative where
the isolation model permits them.

Linux TUN is an explicitly privileged, system-wide capability:

- membership of `mihomo-tun` controls eligibility;
- one root-owned lock is acquired non-blockingly by the first eligible user who
  enables TUN;
- the holder keeps the lock only while its verified TUN is live; process exit
  releases the kernel lock;
- other users can see the holder but cannot stop that TUN; root retains service
  administration authority;
- the active TUN captures host traffic and system DNS, so it intentionally
  affects all accounts;
- installer-managed capabilities and polkit rules allow eligible services to
  create their own TUN interface and configure systemd-resolved without an
  interactive prompt;
- the same group may start the root update unit, which upgrades the shared
  program for every account to the latest official release.

If a saved instance starts while another user holds the system TUN, its saved TUN
state is yielded and the instance starts without TUN. Ordinary mixed proxy
listeners remain available for every user regardless of TUN ownership.

## Persistence and ownership

The main data directory contains the profile catalog and sources, typed settings,
runtime revisions/manifest, recovery journals, management token, managed core,
resource/cache files, selection records and optional retained backups. A
directory lock prevents two supervisors from owning the same state.

Shared installation follows XDG locations for per-user config and data. The
installer records resolved absolute paths so systemd startup does not depend on
an interactive shell environment. Native NixOS installation uses immutable CI release bundles in `/nix/store`.
`packages.default`/`server-bin` fetch a version/hash-pinned x86_64 Linux release,
verify its published checksums, patch ELF interpreters/shebangs, add the current
Nix integration scripts, and regenerate the resource manifest's core hash after
fixup. `server-source` is an explicit alternative, building locked Rust and npm
sources and fetching the pinned core. Source builds inject a version/revision;
release binaries keep the version embedded by CI. The release workflow advances
the server pin on main after assets are published.

The NixOS module installs CLI/helper resources, declares systemd **user** units for
selected normal users, manages lingering, slot directories and the shared TUN
lock with tmpfiles, and grants TUN/DNS access through `security.wrappers` and
the installer's polkit rule (each member's own `ms<uid>` link only). It does not
grant access to the installer update unit. Wrapped scripts put `/run/wrappers/bin`
first, so the launcher's group re-entry uses NixOS's setuid `sg`. A system refresh
unit reloads running user managers and restarts declared instances not already
running the package when the package or unit changes, including rollback, and
reports a user unit that shadows the module's. Each instance retains its XDG data,
settings and token. Autostart belongs to the NixOS `users` option. Runtime core
upgrades stay per-user; a system rollback does not restore mutable core/data.

`nix-installation.json` beside the executable identifies package ownership,
independently of the host distribution. Nix-owned CLI update/uninstall and API
upgrade/autostart changes are refused before invoking the installer or systemctl;
Web hints occupy existing fields/tooltips. The CLI wrapper also guards old CI
binaries. The desktop client's own flake writes the same marker beside its
executable and points its lifecycle helper at this flake's `server-bin`. New API/Web ownership hints
require a release containing that code.
Script installations (including `/opt` installations on NixOS) retain their
installer upgrade behavior, except that the installer refuses to install or
upgrade once the module provides the user unit (`--uninstall` remains). Migration explicitly removes old per-user units,
drop-ins and command links that would shadow Nix units or binaries; it never
silently deletes user state. See [NIXOS.md](NIXOS.md).

TUN capability launcher discovery supports explicit `MIHOMO_TUN_EXEC`,
`/run/wrappers/bin/mihomo-tun-exec` (NixOS `security.wrappers`), and co-located
bundle binaries. The capable executable is an unwrapped copy, so capability
execution never passes through a shell wrapper.
Upgrades preserve user data and individually upgraded cores; uninstall and purge remain
distinct operations.

## Supported and deferred scope

| Area | Status |
| --- | --- |
| Linux x86_64 service, Web management and systemd installation | Implemented and verified |
| Profiles, enhancements, settings, rules/providers, selection and delay tests | Implemented and verified |
| Region-restriction (unlock) and IP location tests through the core's listener | Implemented |
| Resource inventory, Geo/provider lifecycle and live readback | Implemented and verified |
| Stable/Alpha core management and transactional backup/restore | Implemented and verified |
| Multi-user isolation and first-come system-wide TUN | Implemented and verified |
| Simplified/traditional Chinese and English browser UI; service locale catalog | Implemented |
| Linux x86_64 desktop client (local instance; deb/RPM/AppImage/Nix Flake; separate repository) | Implemented |
| Desktop client management of remote instances | Deferred |
| aarch64/musl bundles, server deb/RPM and containers | Deferred |
| Windows service/Named Pipe and native macOS deployment validation | Deferred |
| SOCKS/PAC subscription download routes and full connection dashboard | Deferred |
| Backup automation, WebDAV and backup UI | Deferred |
| Other desktop features beyond the client above | Deferred |

The original design rationale is retained in [../headless.md](../headless.md), but
that document may describe planned or historical boundaries. This file is the
authority for the implemented architecture.

## Documentation maintenance

Update this document only when a change alters a component boundary, ownership
rule, durable transaction, security invariant, deployment model, supported scope
or other long-lived architecture fact. Replace outdated statements instead of
appending task narratives. Commit-level implementation notes, exact test counts,
temporary priorities and superseded designs belong in Git history or the relevant
change report.
