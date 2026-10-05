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
| `crates/management-client` | Client side of the management API shared by the command line and the desktop client: locating this user's instance from systemd, authenticated commands with typed errors, and pure readings of proxy groups, subscriptions, mode and TUN. |
| `crates/clash-verge-*` | Small reusable upstream components: drafts, admission limiting, locale resources and Unix signal handling. |
| `service` | Axum management surface, authentication, durable stores, core actor, downloads, resource/core updates, backup/restore, multi-user isolation and shutdown. The same executable is the `mihomo-server` command-line client (`service/src/cli`): `serve` (or a leading service option) runs the service, any other command is a client. |
| `web` | React browser client. It talks only to the Rust service, keeps session state local and shares the interface language through the instance's preferences. |
| `desktop` | Optional Tauri 2 desktop client (separate Cargo workspace, own lockfile): a window showing the service's own Web UI, a tray menu for mode/TUN/node/subscription control and the service lifecycle, and detection, installation and start of the local instance. Never required by the service. |
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

The `mihomo-server` command is a second client of the same authenticated command
API, with no private path into the service. It locates the invoking user's
instance from the systemd unit's main process (listener, public origin and data
directory from its command line), so it sends the Host the service authorizes and
reads the token the service wrote. Service lifecycle is delegated to the release's
`mihomo-server-user` helper and program updates/uninstall to the installer, so
systemd and root actions keep a single implementation.

The desktop client is a third client of the same API and is installed and
versioned independently of the service. It manages only the invoking user's local
instance, found the same way as by the command line, and re-reads the token file
on every connection instead of storing it. Its tray polls `status` and
`proxy_access`, and re-reads proxies, subscriptions and multi-user facts only when
they change or once a minute. Of the WebSocket feeds it uses only
`/api/streams/preferences`, never `/api/events`, which carries every core log
line. The management window loads the service origin directly and
logs in through the URL fragment, so the browser policy (same origin, no CORS)
is unchanged. It navigates only within that origin; a `target="_blank"` link
to the same origin (the Service page's tokenized management address) goes to
the system browser through `xdg-open`, and no other new window opens. That window may call only the client's own start-at-login commands
(`client_autostart`, `set_client_autostart`, an XDG autostart entry that runs
`mihomo-server-desktop --hidden`), through a capability added at runtime for
exactly the service origin it opened. Only the bundled status page may call the
other commands (detect, install, start, open), which the capability ACL enforces
per window and origin. The Web settings page shows the client switch only when
the client identifies itself (`__MIHOMO_DESKTOP_VERSION__`); the service switch
enables or disables the service's own systemd unit (`set_service_autostart`)
without stopping it, and is unavailable for transient units or a directly
started service. Installation runs the published installer,
whose root step uses polkit (`MIHOMO_INSTALL_ELEVATE=pkexec`) instead of a
terminal sudo prompt. The status page's settings view (gear button) offers the
interface language and a temporary proxy for installation. The proxy is kept
in memory only and used only by the install task: for the installer download
and, through `http(s)_proxy`/`all_proxy`, for the installer's own downloads,
which all run as the user before the polkit step; its test fetches that same
installer URL through it. The page's output panel shows the installer's output
as it is produced: the client sets `MIHOMO_INSTALL_PROGRESS=1` so curl draws its
download bar without a terminal and updates that line in place on each carriage
return, and the installer streams the user activation step line by line instead of
printing it once it has finished.

The client keeps three things apart: itself (versioned on its own), the
systemd service (`mihomo-server`, started, stopped and restarted through the
`mihomo-server-user` helper like the command line does) and the Mihomo core
the service supervises. The tray manages the first two and proxy settings
through the API; it never starts or stops the core and never presents the
core's version or phase as the service's. The service version shown is read
from the running service binary (`/proc/<MainPID>/exe --version`), since the
API's `status` reports the core. The tray uses tray-icon's StatusNotifierItem
(`ksni`) backend instead of Tauri's default libappindicator, which reports no
clicks and shows no tooltip: a left click opens the management window (or the
status page), the menu is on right click, and its header is a single line of
service state. Details go to the tooltip, and failures are also sent as desktop
notifications (freedesktop D-Bus).

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
an interactive shell environment. Upgrades preserve user data and individually
upgraded cores; uninstall and purge remain distinct operations.

## Supported and deferred scope

| Area | Status |
| --- | --- |
| Linux x86_64 service, Web management and systemd installation | Implemented and verified |
| Profiles, enhancements, settings, rules/providers, selection and delay tests | Implemented and verified |
| Resource inventory, Geo/provider lifecycle and live readback | Implemented and verified |
| Stable/Alpha core management and transactional backup/restore | Implemented and verified |
| Multi-user isolation and first-come system-wide TUN | Implemented and verified |
| Simplified/traditional Chinese and English browser UI; service locale catalog | Implemented |
| Linux x86_64 desktop client (local instance; deb/RPM/AppImage) | Implemented |
| Desktop client management of remote instances | Deferred |
| aarch64/musl bundles, server deb/RPM and containers | Deferred |
| Windows service/Named Pipe and native macOS deployment validation | Deferred |
| SOCKS/PAC subscription download routes and full connection dashboard | Deferred |
| Backup automation, WebDAV and backup UI | Deferred |
| Media-unlock and other desktop features beyond the client above | Deferred |

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
