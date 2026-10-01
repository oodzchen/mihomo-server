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
- **Deferred**: planned capabilities or platform compatibility that have been postponed (e.g., Windows compatibility).

> [!NOTE]
> **Windows compatibility deferred**: Windows-related feature plans (including Windows Named Pipe runtime validation, native Windows system proxy discovery, and Windows SCM service integration) are commented out and deferred to focus on Linux and core headless service stability.

A copied source file, a successful build, or an empty route does not by itself
establish a working integration. Mark partially migrated components explicitly.

## Active priorities (user override)

The user has explicitly narrowed the current delivery scope. This order overrides
older increment next-task notes and the previous broad feature-expansion plan:

1. **P1 — Configuration and resource management.** Complete authoritative service
   settings, Geo/provider resource paths, discovery, validation and lifecycle,
   configuration generation/application/recovery, and the corresponding Web
   management. Preserve real subscription data and verify usable proxy traffic.
2. **P2 — Rules, providers and delay testing.** Connect the retained client methods
   to authenticated service commands and usable Web views; verify actual core
   readback, refresh/reload behavior, delay results and failure handling.
3. **P3 — i18n and signals.** Migrate backend/browser language resources and signal
   behavior needed by this Linux service. Keep browser language independent of
   service-global state and preserve the already working unified Unix shutdown.
4. **P4 — Linux packaging and actual systemd installation.** Deliver a reproducible
   Linux bundle and install/run a systemd unit. Verify boot/start, stop/restart,
   logs, authentication, persistent state, Mihomo child reaping and real proxy
   traffic through the installed service. A template or static unit check alone
   does not complete this priority.

The user's original numbering was 1, 2, 4, 5; the sequence above normalizes labels
without adding another priority. Finish reviewable increments in this order.
Only supporting work necessary for these priorities belongs in the active scope.

All other unfinished work is **Deferred until the user expands scope**: further
backup automation/change triggers/retention/WebDAV/backup UI, media unlock,
SOCKS/PAC, full connection dashboards, unrelated shared-component/domain-event
expansion, containers, non-Linux releases/services and external publication.
Windows compatibility remains deferred. Existing verified backup/upgrade and
other delivered functionality is retained; it does not justify expanding it now.
Do not resume backup work based on an older chapter's next-task paragraph.

**Latest completed task:** GitHub Actions release pipeline (`ci.yml` full tests on every push, `release.yml` tarball bundle on `v*` tags), pinned core file `deploy/core-pin.json`, and remote one-shot installer `scripts/install_remote.sh` with locally verified end-to-end install (see "Increment: CI release pipeline and remote installer").
**Previous completed task:** Code-quality refactor step 4 — real module directories for backup, core release/upgrade and Geo (see below).
**Previous completed task:** Persistent login. The web UI caches the management token in sessionStorage and re-validates it on load, so refreshing keeps the session; logout/401 clears it.  The e2e restart test now asserts refresh keeps the session and logout+refresh returns to the login page.

**Previous completed task:** Minimalist centered login page layout redesign. Replaced the split-screen layout and promotional copy (`.login-art` with marketing slogans/intros) with a clean, centered minimalist card layout. The login view centers the card vertically and horizontally in the viewport with top title (`连接你的服务`), concise explanation (`loginHelp`), and centered login box (`token` password input, submit button, and data directory hint). Moved interface language selection cleanly to the top-right corner, ensuring responsive display on both desktop and mobile viewports while maintaining complete e2e test compatibility.
**Next implementation task:** Maintain deployed Linux service, support user feature queries, and expand deferred capabilities upon request.

## Code-quality refactor (behavior-preserving)

Structural cleanup with no business-logic change. Each step keeps the Rust
suite, the real-core `--ignored` suite and the real-node data checks at the
pre-refactor baseline.

1. **Shared filesystem primitives — done.** `service/src/secure_fs.rs` now owns
   the raw `libc` descriptor calls (`openat`, `unlinkat`, `mkdirat`,
   `renameat2(RENAME_NOREPLACE)`, `fchmod`, `flock`, `fdopendir/readdir`,
   `geteuid`) with `SAFETY` notes, plus the previously duplicated SHA-256/hex,
   directory fsync and owner-only `create_new` helpers. Backup, Geo, core
   release/upgrade and TUN modules call these; each keeps its own ownership and
   permission policy and its original error messages. `headless-core`
   `profile_store` shares one SHA-256 helper between import and restore journals.

2. **Core manager domain modules — done.** The 4,100-line
   `service/src/core_manager.rs` is now `service/src/core_manager/`:

   ```text
   core_manager/
   ├── mod.rs        Public types, CoreOptions, CoreManager::spawn/call, Actor state, run loop
   ├── messages.rs   Operation, CommandMessage (+ fail), ProfileChange, ConfigCandidate
   ├── dispatch.rs   Journal recovery gate, record_error/publish_profiles, command routing
   ├── lifecycle.rs  start/stop/reload/exit observation/recovery, Operation execution
   ├── apply.rs      stage/apply/apply_with_change transactions and rollback
   ├── profiles.rs   imports, raw edits, enhancements, remote refresh, deletion
   ├── settings.rs   settings/DNS decisions, connection-settings readback
   ├── selection.rs  node selection and saved-selection restoration
   ├── geo.rs        Geo seeds, online updates, validation, resource inventory
   ├── upgrade.rs    core release discovery, staging, upgrade and activation
   ├── backup.rs     export, retained storage, validation and restore
   ├── scheduler.rs  scheduled subscription updates (unchanged)
   └── upgrade_tests.rs
   ```

   Each domain file holds both the `CoreManager` API methods and the `Actor`
   handlers for that domain. The command match left `tokio::select!` (so
   rustfmt formats it), recovery failures reply through one
   `CommandMessage::fail`, API methods share `CoreManager::call`, and
   `execute` has one arm per operation. Method bodies, error messages and the
   serialized actor semantics are unchanged; the move was verified line by line.
3. **Typed Web connection state and per-page modules — done.** WebSocket state
   is the `Connection` union (`connecting | connected | reconnecting |
   unauthorized | badData`) exported by `web/src/api.ts`; it doubles as the
   i18n key, so components compare against stable values instead of Chinese
   display text and every language shows the translated label. The fallback
   request error is localized (`requestFailed`). `web/src/main.tsx` keeps the
   app shell (App, Login, Manager, routing); pages moved verbatim to
   `overview.tsx`, `profiles.tsx`, `enhancement-editors.tsx`, `config.tsx`,
   `proxies.tsx` and `logs.tsx`, with shared `describe`/`bytes` in `format.ts`.
4. **Real module directories — done.** Modules that were nested through
   `#[path]` now live in matching directories: `service/src/backup/`
   (`mod`, `inspect`, `candidates`, `restore`, `storage` and tests),
   `service/src/core_release/` (`mod`, `stage`, `transport` and tests) and
   `service/src/core_upgrade/`. Geo modules are grouped as `service/src/geo/`
   (`dat`, `live`, `online`, `resources`, `settings`, `update`, `validation`);
   their crate paths changed from `crate::geo_*`/`crate::dat_validation` to
   `crate::geo::*` with unchanged visibility and platform gating.

Not changed on purpose: the remaining hard-coded Chinese text in the settings
pages (translation content, not a refactor), and this document's long
increment history (the autonomous workbench appends to it).

## Recent update: Multi-agent autonomous workbench (Codex & Antigravity CLI)

The autonomous workbench (`automation/`) now supports both **OpenAI Codex CLI**
and **Google Antigravity CLI (`agy`)** as interchangeable autonomous agent backends:

1. **Agent selection**: `--agent codex` (default) and `--agent agy` / `--agent antigravity`
   select the respective agent CLI engine. `run_autonomous.sh` symlink provides a
   unified invocation entry point.
2. **Permission, sandbox & unattended execution**: Both engines run inside Linux isolation
   sandboxes by default (Codex via Bubblewrap `workspace-write`, Antigravity via Linux namespace
   `--sandbox` with read-only host protection). Auto-approvals (`approval_policy=never` /
   `--dangerously-skip-permissions`) and `--add-dir ../clash-verge-rev` upstream cross-repo mounts
   are preserved, with optional `--no-sandbox` bypass.
3. **Stream formatting & folding**: `format_codex_stream.py` auto-detects stream type,
   providing native parsing for Antigravity's `stream-json` NDJSON events while retaining
   full state-machine support for Codex text streams. Both produce consistent dynamic
   Spinner loading, command/output folding, code patch summaries, and turn elapsed time.
4. **Session persistence & isolation**: Sessions are tracked per agent (`.session_id_codex`
   and `.session_id_agy`) as well as the active `.session_id`, preventing cross-engine
   session collision when switching agents.
5. **Rate-limit detection & smart cooldown**: Expanded regex and parser support both OpenAI
   usage limits and Google/Gemini quota exhaustion, with accurate time-only and relative
   countdown parsing (e.g. `try again at 4:17 PM`) to eliminate blind 5-hour waits.
6. **Host Git atomic commits & progress tracking**: Both engines format standard
   `COMMIT_START ... COMMIT_END` blocks for host-level conventional commits and enforce
   `docs/ARCHITECTURE.md` synchronization and `$COMPLETION_FLAG` guards identically.
7. **Cross-Agent Handover Bridge**: Resolves session discontinuity when switching agents (e.g.,
   when Codex hits its 5-hour quota). The host scheduler automatically synthesizes an in-flight
   worktree diff (`git status -s`), the predecessor's latest turn summary (`turn_*_last_msg.txt`)
   with explicit next-task directives, and recent Git commits into a structured handover briefing.
   This ensures the successor agent (`agy`) picks up in-progress code immediately, verifies build
   and tests, and maintains identical development cadence without discarding work.
8. **Development rhythm & turn cadence injection**: Prompts enforce a strict incremental
   rhythm based on prior turn history: single subtask focus per turn, implement & verify,
   synchronize `docs/ARCHITECTURE.md`, output a mandatory 4-part summary (Commit block ->
   Feature summary -> Verification results -> Next task directive), and hand off to the host
   for atomic Git commit before automatically launching the next turn until all tasks complete.

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
│   ├── Shared upstream components                   [Implemented; Linux verified]
│   │   ├── Thirteen i18n locale assets / aliases / explicit lookup, browser language switcher (zh/zhtw/en), and Accept-Language negotiation [Implemented; Linux verified]
│   │   ├── Unix SIGTERM/SIGINT/SIGHUP listener and shutdown latch [Migrated + headless adaptation; Linux verified]
│   │   └── Additional logging / media unlock         [Deferred; outside active scope]
│   ├── mihomo-client/                               [Migrated; Linux verified]
│   │   ├── Unix socket / explicit loopback HTTP      [Migrated]
│   │   ├── API methods, response models, errors      [Migrated]
│   │   ├── Presence-preserving connection/outbound/download GET /configs projection [Implemented; Linux verified]
│   │   ├── Presence-preserving Geo GET /configs projection and URL aliases [Implemented; Linux verified]
│   │   └── Realtime feeds, cancellation, reconnect   [Migrated]
<!--│   │   └── Windows Named Pipe runtime validation    [Deferred; code retained; Windows compatibility postponed] -->
│   └── headless-core/                               [Implemented; Linux verified]
│       ├── Draft and limiter type re-exports        [Implemented]
│       ├── Subscription models and YAML schema     [Migrated]
│       ├── Runtime revisions, manifest, commit/recovery
│       │                                            [Implemented]
│       ├── Profile catalog, files, local import     [Implemented; upstream schema]
│       ├── Versioned settings store / explicit runtime fields [Implemented; Linux verified]
│       ├── Settings/runtime journal / interrupted-update recovery [Implemented; Linux verified]
│       ├── Full service settings and resource paths  [Implemented; Linux verified]
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
│       ├── Typed DNS/TUN, resolver policies/fallback filter, host-use booleans / authority [Implemented; Linux verified]
│       ├── DNS page H3/rule-respecting/fake-IP filter-mode controls [Implemented; Linux verified]
│       ├── settings/dns_policy.rs / strict policy values / per-leaf fallback ownership [Implemented; Linux verified]
│       ├── Typed Geo fields incl. geosite matcher / per-URL authority / bounds / schema-one recovery [Implemented; Linux verified]
│       ├── TCP concurrency / process mode authority / schema-one recovery [Implemented; Linux verified]
│       ├── Signed TCP keep-alive durations / disable authority / schema-one recovery [Implemented; Linux verified]
│       ├── Pure TUN/DNS derivation / IPv6 range repair [Migrated + staged adaptation; Linux validation]
│       ├── Provider DNS digest / profile preference / session confirmation / hosts-only protection [Migrated + adaptation; Linux verified]
│       ├── Deleted-profile DNS preference / confirmation cleanup [Implemented; recoverable]
│       ├── settings/hosts.rs / strict typed maps / whole-map authority / empty clear / alias-cycle checks [Implemented; Linux verified]
│       ├── Native Linux TUN admission / live config readback [Implemented; Linux verified]
│       ├── Final LAN bind / group cleanup / field order [Migrated + staged adaptation; Linux verified]
│       ├── Outbound interface / Linux routing mark authority / bounds / recovery [Implemented; Linux verified]
│       ├── Global download User-Agent / ETag authority / strict headers / recovery [Implemented; Linux verified]
│       ├── Remaining authoritative settings (bind, auth, LAN ACL, TFO/MPTCP, sniffing) [Implemented; Linux verified]
│       ├── Source-addressed HTTP provider cache identities / implicit paths [Implemented; Linux verified]
│       ├── Runtime YAML + overlay generation        [Implemented; upstream merge reused]
│       ├── Profile enhancement generation          [Implemented; sequences/settings/TUN/DNS/global/profile/final stages; Linux verified]
│       ├── Per-profile node selection records       [Implemented; upstream schema]
│       ├── Provider path authority / normalized destinations / SHA-256 cache allocation [Migrated + service adaptation; Linux verified]
│       ├── Remaining Geo lifecycle / resource settings [Implemented; Linux verified]
│       ├── Rule and provider operation models        [Implemented; Linux verified]
│       ├── Proxy provider and delay operation models [Implemented; Linux verified]
│       ├── Immutable revision / orphan file garbage collection [Implemented; Linux verified]
│       ├── Timed update metadata / saved refresh source [Migrated + service scheduler]
│       ├── Backup manifest / bounded entries / inspection, validation, runtime policy and restore receipt models [Implemented; upstream ZIP adaptation]
│       ├── Opaque restore plan / durable catalog-settings journal / runtime commit recovery [Implemented; Linux verified]
│       ├── Retained backup metadata / list / create-delete receipt models [Implemented]
│       ├── Full resource settings                       [Implemented; Linux verified]
│       └── Automatic retention models               [Deferred; outside active scope]
├── service/                                         [Implemented; Linux verified]
│   ├── Persistent foreground entry point            [Implemented]
│   ├── Binary/data/config/import args, directory lock [Implemented]
│   ├── Listen/public-origin args and private token [Implemented; Linux verified]
│   ├── Built Web directory argument               [Implemented; --web-dir]
│   ├── Pinned resource directory / persistent managed core [Implemented; Linux]
│   ├── Startup settings snapshot / candidate authority [Implemented; Linux verified]
│   ├── Actor settings read/replace / coordinated apply and rollback [Implemented; Linux verified]
│   ├── DNS/TUN/hosts, resolver policy/fallback generation and settings transactions [Implemented; Linux verified]
│   ├── DNS page H3/rule-respecting/filter-mode API validation/apply [Implemented; Linux verified]
│   ├── native_tun.rs / device admission, interface/route validation and live TUN config readback [Implemented; Linux verified]
│   ├── Raw/enhanced candidate phases / single TUN derivation [Implemented; Linux validation]
│   ├── DNS/hosts conflict commands / scoped confirmation / coordinated auto-disable [Implemented; Linux verified]
│   ├── Final candidate LAN/group normalization after authority [Implemented; Linux verified]
│   ├── Geo/provider resources / full settings       [Implemented; Linux verified]
│   │   ├── Committed resource inventory / confined metadata / shared-path diagnostics [Implemented; Linux verified]
│   │   ├── Final candidate provider normalization / conflict allocation / preserved source YAML [Implemented; Linux verified]
│   │   ├── Probe/start/reload resource-path checks / service-file protection [Implemented; Linux verified]
│   │   ├── Cross-revision HTTP cache ownership / header-parser separation / legacy reapply guard [Implemented; Linux verified]
│   │   ├── Pinned Geo seed schema / bounded staging / no-overwrite bootstrap / orphan recovery [Implemented; Linux verified]
│   │   ├── Read-only MMDB verification / pinned parser / metadata-only compatibility outcome [Implemented; Linux verified]
│   │   ├── Stopped-core pinned MMDB replacement / digest guards / atomic commit / orphan recovery [Implemented; Linux verified]
│   │   ├── Geo actor settings/config/core comparison / presence-preserving bounded readback / URL model aliases [Implemented; Linux verified]
│   │   ├── Connection/outbound/download comparison / nine presence-preserving fields / shared snapshot envelope [Implemented; Linux verified]
│   │   ├── dat_validation.rs / bounded protobuf / CIDR-domain-attribute checks / CN diagnostics [Implemented; Linux verified]
│   │   ├── Read-only DAT snapshots / aggregate reports / core compatibility warning [Implemented; Linux verified]
│   │   ├── Stopped-core pinned DAT installation / four-mode core load proof / digest guards / orphan recovery [Implemented; Linux verified]
│   │   ├── geo_online.rs / committed-source inspection / 128 MiB direct-system-managed download / TLS fallback / optional pins [Implemented; Linux verified]
│   │   ├── Stopped-core online MMDB/DAT update / staged validation / four-mode DAT load proof / digest guards [Implemented; Linux verified]
│   │   ├── geo_live.rs / durable rollback journal / startup recovery / guarded cleanup [Implemented; Linux verified]
│   │   ├── Running-core online Geo replacement / verified restart / rollback and crash recovery [Implemented; Linux verified]
│   │   ├── Online Geo managed/system proxy route choice and TLS retry parity [Implemented; Linux verified]
│   │   ├── resource_inventory.rs / effective automatic-update state / freshness calculation / provider intervals [Implemented; Linux verified]
│   │   └── Native Linux TUN interface and route verification [Implemented; Linux verified]
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
│   │   ├── Native macOS proxy discovery runtime validation [Deferred; library code retained]
<!--│   │   ├── Native Windows proxy discovery runtime validation [Deferred; Windows compatibility postponed] -->
│   │   ├── TLS platform/static roots / explicit certificate option [Migrated + adaptation; Linux verified]
│   │   ├── Scheduled refresh / retirement / bounded workers / drain [Migrated + adaptation; Linux verified]
│   │   └── SOCKS/PAC                               [Deferred; outside active scope]
│   ├── Core release preparation / stable and Alpha activation [Implemented; Linux x86_64 verified]
│   │   ├── Official stable/Alpha metadata and platform asset [Implemented; Linux x86_64]
│   │   ├── Bounded compressed download / SHA-256 / private atomic cache [Implemented]
│   │   ├── Authenticated preparation/readback / cancellation [Implemented]
│   │   ├── Bounded gzip / ELF / version / configuration probes [Implemented; stable/Alpha Linux x86_64]
│   │   ├── Actor snapshot / staged manifest / verified restart readback [Implemented]
│   │   ├── Actor activation / live version and port checks / rollback [Implemented; Linux x86_64]
│   │   ├── Durable switch journal / installation receipt / startup recovery [Implemented; Linux x86_64]
│   │   ├── Stable upstream force/no-op adapter / upgrade Web workflow [Implemented; Linux x86_64]
│   │   ├── Repair of unreadable/empty previous core [Implemented; Linux x86_64]
│   │   ├── Managed/system/direct routing / verified static-root TLS retry [Implemented; Linux verified]
│   │   ├── Alpha compressed preparation / authenticated readback [Implemented; Linux x86_64]
│   │   ├── Alpha executable/version/config staging / private proof / readback [Implemented; Linux x86_64]
│   │   ├── Alpha activation / receipts / durable rollback / startup recovery [Implemented; Linux x86_64]
│   │   ├── Alpha upstream force/no-op adapter / channel-aware Web workflow [Implemented; Linux x86_64]
│   │   └── Other core upgrade targets [Deferred; outside active scope]
│   ├── Node selection / unfix / persistence rollback [Implemented; Linux verified]
│   ├── Selection reconciliation and restoration    [Migrated + actor adaptation]
│   │   └── Startup keep-records, apply repair, bounded provider retries
│   ├── Local backup export, inspection, restoration and retained storage [Implemented; Linux verified]
│   │   ├── Actor snapshot / bounded ZIP / digest manifest [Implemented]
│   │   ├── Authenticated binary download / single body-owned admission [Implemented]
│   │   ├── Strict ZIP structure / CRC / SHA-256 / catalog and settings references [Implemented; Linux verified]
│   │   ├── Authenticated read-only binary inspection / pre-upload shared admission [Implemented; Linux verified]
│   │   ├── Disposable restore candidate / archived runtime / active enhancement validation [Implemented; Linux verified]
│   │   ├── Actor-owned core snapshot / isolated Geo data / probe cancellation and cleanup [Implemented; Linux verified]
│   │   ├── Durable catalog/settings restore journal / runtime-marker recovery [Implemented; Linux verified]
│   │   ├── Startup + actor recovery before DNS pruning/defaults/current mirror [Implemented; Linux verified]
│   │   ├── Explicit running/stopped restore upload / DNS policy / durable receipt [Implemented; Linux verified]
│   │   ├── Restore preparation/core-I/O cancellation / phase checks / committed cleanup [Implemented; Linux verified]
│   │   ├── Running-core restore reload/restart/apply rollback / saved-node reconciliation [Implemented; Linux verified]
│   │   ├── Scoped candidate leases / bounded abrupt-termination orphan cleanup [Implemented; Linux verified]
│   │   ├── Private retained archives / create-list-download-delete / partial recovery [Implemented; Linux verified]
│   │   └── Automatic retention / schedule / change triggers / WebDAV / UI [Deferred; outside active scope]
│   ├── Full application context and domain events  [Deferred; expand only as needed for P1–P3]
│   ├── Sole Mihomo lifecycle manager                [Implemented; Linux verified]
│   │   ├── Start, readiness, stop, restart, recovery, reap
│   │   ├── Linux core/validator parent-death termination [Implemented; Linux verified]
│   │   ├── YAML / Mihomo -t validation and cancellation [Implemented]
│   │   ├── Serialized reload, restart fallback, rollback [Implemented]
│   │   ├── Runtime commit and interrupted-apply recovery [Implemented]
│   │   ├── Active profile + runtime commit/recovery [Implemented]
│   │   ├── Linked/global enhancement validation/apply/rollback [Implemented; Linux verified]
│   │   └── Full enhancement/resource transaction    [Implemented; Linux verified]
│   ├── Axum management API / command adapters       [Implemented; MVP allowlist]
│   │   ├── State, logs, profiles, config, proxies queries [Implemented]
│   │   ├── Lifecycle, YAML import/edit/overlay, profile edit/delete/import/refresh, linked read/set/clear, global read/set/reset, settings read/replace, profile DNS read/set, raw profile read/edit and node selection [Implemented]
│   │   ├── Stable core query / preparation / staging / activation / installation/version readback / force-no-op; Alpha query / compressed preparation / executable staging / activation / installation readback / force-no-op [Implemented; Linux x86_64]
│   │   ├── Authenticated POST /api/backup ZIP export [Implemented; Linux verified]
│   │   ├── Authenticated POST /api/backup/inspect validation report [Implemented; Linux verified]
│   │   ├── Authenticated POST /api/backup/validate restore rehearsal [Implemented; Linux verified]
│   │   ├── Authenticated POST /api/backup/restore running/stopped restoration [Implemented; Linux verified]
│   │   ├── Authenticated POST/GET /api/backups and GET/DELETE /api/backups/{id} [Implemented; Linux verified]
│   │   ├── Rules and rule-provider query and update commands [Implemented; Linux verified]
│   │   └── Proxy providers and group/node delay commands [Implemented; Linux verified]
│   ├── HTTP bearer / WS first-frame auth, Host/Origin controls [Implemented; Linux verified]
│   ├── WebSocket events and realtime forwarding     [Implemented; Linux verified]
│   │   ├── State/profile snapshots, watches, log tail/reset [Implemented]
│   │   ├── Traffic, memory, connections/count, core logs [Implemented]
│   │   └── Per-session cancellation, bounded queues/retry/drain [Implemented]
│   ├── Web static assets and scoped SPA fallback    [Implemented; Linux verified]
│   ├── Unix SIGINT/SIGTERM/SIGHUP and unified shutdown [Implemented; Linux verified]
│   ├── Localized service messages                   [Implemented; Linux verified]
│   ├── User systemd unit and management CLI        [Implemented; Linux verified]
│   └── Other platform service integration           [Deferred; Linux only]
<!--│   └── Windows SCM service integration              [Deferred; Windows compatibility postponed] -->
├── web/                                             [Implemented; Linux verified]
│   ├── React build, login and responsive layout     [Implemented; MVP]
│   ├── HTTP commands, WebSocket events/feed adapters [Implemented; MVP allowlist]
│   │   ├── Rules and rule-provider command views    [Implemented; Linux verified]
│   │   └── Proxy providers and delay command views  [Implemented; Linux verified]
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
│   ├── Rules and rule-provider management page      [Implemented; Linux verified]
│   ├── Proxy providers and node delay views         [Implemented; Linux verified]
│   ├── Browser-owned zh/zhtw/en language selection, login/navigation/core shell/overview copy [Implemented; P3; browser verified]
│   ├── Config editor zh/en copy and draft-safe switching [Implemented; P3; browser verified]
│   ├── Profile list zh/en labels/actions/deletion confirmation [Implemented; P3; browser verified]
│   ├── Local YAML and remote URL import forms / file-size feedback [Implemented; P3; browser verified]
│   ├── Profile metadata editor zh/en fields/options/draft-safe switching [Implemented; P3; browser verified]
│   ├── Raw subscription YAML editor zh/en feedback/confirmation [Implemented; P3; browser verified]
│   ├── Profile-linked merge editor zh/en YAML controls [Implemented; P3; browser verified]
│   ├── Profile-linked sequence editor zh/en types/YAML controls [Implemented; P3; browser verified]
│   ├── Profile-linked script editor zh/en JavaScript controls [Implemented; P3; browser verified]
│   ├── Global merge editor zh/en YAML controls and reset confirmation [Implemented; P3; browser verified]
│   ├── Global script editor zh/en JavaScript controls and reset confirmation [Implemented; P3; browser verified]
│   ├── Proxy node list zh/en selection controls and group status [Implemented; P3; browser verified]
│   ├── Proxy delay-test URL/actions/result badges zh/en [Implemented; P3; browser verified]
│   ├── Proxy-provider inventory/update/healthcheck controls zh/en [Implemented; P3; browser verified]
│   ├── Rule list zh/en search, counts, table and empty/loading states [Implemented; P3; browser verified]
│   ├── Rule-provider inventory/update controls and status feedback zh/en [Implemented; P3; browser verified]
│   ├── Log view zh/en heading, filter input, clear action and empty/unmatched states [Implemented; P3; browser verified]
│   ├── Core upgrade view zh/en channels, release info, install records and action dialogs [Implemented; P3; browser verified]
│   ├── Resource views zh/en inventory, auto-update policy, validation and seed/online actions [Implemented; P3; browser verified]
│   ├── Additional languages and service-message localization [Implemented; P3; zh/zhtw/en frontend and service messages verified]
│   ├── Full connection dashboards                   [Deferred; outside active scope]
│   ├── Runtime settings editor / inheritance / readback [Implemented; Linux verified]
│   ├── TCP concurrency / process mode / keep-alive editor / shared comparison / retry [Implemented; Linux verified]
│   ├── outbound-settings.tsx / interface ownership / Linux mark editor / readback [Implemented; Linux verified]
│   ├── download-settings.tsx / User-Agent ownership / ETag editor / readback [Implemented; Linux verified]
│   ├── DNS/TUN editor / resolver policy and fallback JSON / nested inheritance / readback [Implemented; Linux verified]
│   ├── DNS H3/rule-respecting/filter-mode editor [Implemented; Linux verified]
│   ├── hosts-settings.tsx / typed JSON editor / explicit empty / canonical save comparison / snapshot [Implemented; Linux verified]
│   ├── Provider DNS confirmation / cancellation / reconnect reconciliation [Implemented; Linux verified]
│   ├── Stable/Alpha channel selection / core upgrade / broken-core repair / force confirmation / installation readback / retry [Implemented; Linux x86_64]
│   ├── Geo/Provider inventory / metadata states / auto-update state / freshness diagnostics / refresh and retry [Implemented; Linux verified]
│   ├── Explicit MMDB/DAT checks / aggregate diagnostics / compatibility warnings / stale-result clearing [Implemented; Linux verified]
│   ├── Pinned MMDB/DAT bundle inspection / stopped-state install / DAT core load proof / MMDB metadata-only choice [Implemented; Linux verified]
│   ├── Configured-source Geo inspection / explicit download route and TLS choice / stopped and running-core update / fresh-hash retry [Implemented; Linux verified]
│   ├── Geo field editor incl. geosite matcher / per-URL inheritance / saved-configured-actual readback / retry [Implemented; Linux verified]
│   ├── Remaining full settings/resource lifecycle UI [Implemented; browser verified]
│   └── Backup UI [Deferred; outside active scope]
├── Release and deployment                           [Implemented; Linux release verified]
│   ├── Linux x86_64 bundle: Rust + independent Mihomo + Web [Implemented]
│   ├── Explicit target/version/SHA-256 resource manifest [Implemented]
│   ├── Writable persistent core initialization      [Implemented; Linux verified]
│   ├── One foreground exec launcher                 [Implemented; Linux verified]
│   ├── Preserve data and existing upgraded core      [Implemented; Linux verified]
│   ├── Managed core installation receipt / interrupted-switch recovery [Implemented; Linux x86_64]
│   ├── Actual Linux systemd installation / lifecycle verification [Implemented; Linux verified]
│   ├── Optional integrity-pinned Geo resources / packager handoff [Implemented; P1 dependency; Linux verified]
│   ├── Full Linux release resource/license inventory [Implemented; Linux verified]
│   ├── Other platforms / containers / Alpha bundle seeds [Deferred]
│   ├── Linux package license inventory              [Implemented; Linux verified]
│   └── External publication                      [Deferred]
├── automation/                                       [Implemented; multi-engine verified]
│   ├── Host autonomous runner / multi-agent engine support (Codex & Antigravity agy) [Implemented; verified]
│   ├── Unified TUI stream formatter / NDJSON & text parsing / output & diff folding [Implemented; verified]
│   ├── Host-level Git conventional commit automation / AGENTS.md compliance [Implemented; verified]
│   ├── Watchdog inactivity monitor / rate-limit detection / cooldown recovery [Implemented; verified]
│   └── Session isolation / .session_id_codex & .session_id_agy persistence [Implemented; verified]
└── Documentation and provenance                     [Implemented; maintained]
    ├── headless.md
    ├── docs/UPSTREAM.md
    ├── docs/ARCHITECTURE.md
    ├── docs/RUNNING.md
    ├── docs/DEPLOYMENT.md
    └── automation/README.md
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
validation. Typed hosts authority and DNS host-use controls are connected with
provider confirmation; native TUN integration and the full resource pipeline
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
<!-- Windows compatibility deferred:
Windows console/pipe/storage behavior remains unverified, and SCM and
abnormal-exit platform cleanup require deployment integration.
-->
Windows-specific compatibility (console, Named Pipe, SCM, storage) is deferred; abnormal-exit platform cleanup requires deployment integration.

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
the management listener. The token is cached in browser sessionStorage (`mihomo.token`, per-tab, never localStorage) and re-validated via `status` on page load, so refresh keeps the session; logout or a 401 clears it.
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
macOS discovery remains library code awaiting runtime verification.
<!-- Windows compatibility deferred: Native Windows proxy discovery remains library code awaiting runtime verification. -->
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

## Previous increment: managed core activation, health checks and durable rollback

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

## Previous increment: stable force/no-op adapter and browser upgrade workflow

Delivery step 7 now exposes upstream-shaped upgrade_clash_core with required
boolean force, returning upgraded/from/to, plus installed_core_version for an
actor-bounded probe of the actual managed executable while running or stopped.
The wrapper resolves latest stable metadata once, checks the installed version
in the actor, and skips download/staging/replacement/restart when already current
and force is false. Force validates and replaces even the same version. Existing
immutable compressed caches may be reused after digest checks.

One shared admission permit spans discovery, preflight, pinned metadata/hash
download and final actor staging/activation. Network work stays outside the
lifecycle actor. On final admission the actor checks version again, uses current
normalized configuration and reuses the durable activation/health/rollback path.
Active switching retains permit ownership through caller disconnect; shutdown
cancels network work and rolls back an uncommitted switch. No source/path/hash
or version overrides are accepted by the latest-stable wrapper.

The new authenticated /core page reads installed version and receipt independently
of running status, checks latest metadata, upgrades or confirms force reinstall,
disables duplicate actions and clears previous results on retry. It rereads
installation information after success/failure, reconnection and navigation.
Errors remain repairable through refresh/retry. Unmanaged launches report that a
managed bundle is required. The scoped server SPA navigation allowlist includes
/core, supporting direct links and browser refresh without broadening API fallback.
The existing Linux MVP and stopped-mode behavior remain.

Verification for this increment:

- Workspace check, formatting and Clippy (`--all-targets -- -D warnings`) pass.
  Regular workspace tests pass 244 cases; all 66 opt-in tests pass with the real
  `/usr/bin/verge-mihomo` and `--test-threads=1`. Web TypeScript/Vite build and
  formatting checks pass.
- New tests cover metadata pinned across a moving latest release, cancellation
  before cache publication, version preflight/final no-op preserving PID/inode,
  revision and receipt, forced failure rollback, and real running/stopped force
  reinstalls with unchanged configuration. The isolated controller fixture uses
  a short enough directory for Linux Unix socket paths.
- The final verified package is
  `target/mihomo-server-linux-x86_64-core-workflow-final`. Its service binary
  matches this round's `target/release/mihomo-server`; every package checksum
  passes and bundled deployment/provenance documents match source. An initial
  packaging attempt selected a stale binary from the explicit-target directory;
  command smoke caught it, and the verified package explicitly selects the newly
  built host release binary. The server's new /core navigation route is checked
  by the existing static-asset/auth-boundary test. All 23 Playwright workflows pass
  against this final bundle, including direct /core navigation, version/receipt
  reads, busy controls, force cancellation/confirmation, obsolete-result clearing,
  failure/retry, actual running/stopped activation and receipt readback on restart.
  Browser release/no-op/error responses use transport fixtures; force success
  delegates to real authenticated staging/activation. The separate official smoke
  covers the actual wrapper's discovery/download/no-op decisions.
- Official-release smoke verifies latest stable `v1.19.31`, default no-op with
  no package downloaded or PID/inode change, forced installation of the official
  22,805,792-byte compressed asset, live health checks, stopped/running intent,
  version/receipt persistence across restart and running force replacement.
- An isolated copy of the actual saved subscription supplies 56 traffic-capable
  nodes. The first node returns HTTPS 204 before and after the actual official
  force wrapper, with restored selection, preserved active UID/configuration,
  new PID/inode and hash-verified receipt. Subscription system-proxy fetching
  still reaches YAML validation. Original `data` hashes remain unchanged and
  automatic refresh is disabled only in the disposable copy. This smoke uses
  ordinary platform roots; core-download static-root fallback remains pending.
- The final process audit finds zero fixture services, cores, script workers or
  probe scripts remaining. Disposable smoke data is removed; original data is
  untouched. No Git metadata was written.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The complete project is not done. Next Delivery step 7 subtask: core-download
managed/system/direct proxy routing with static-root TLS fallback, retaining the
successful metadata route for package download and lifecycle cancellation.
Broken/empty-core repair, Alpha/other targets, native TUN/DNS/hosts/resources,
backups/WebDAV, advanced pages, garbage collection, SOCKS/PAC and additional
platform/deployment integrations remain pending.

## Previous increment: core-download routing and verified TLS fallback

Delivery step 7 now routes stable latest/pinned discovery through managed-core,
system and direct policies in order. Managed endpoints/authentication come from
actual HTTP/Mixed ports and committed configuration with port consistency and
generation/PID/revision checks. Unavailable managed routes are skipped. System
uses validated environment/native discovery with NO_PROXY/global/CGI bypass.
Discovery errors advance policies; each has one 20-second budget including TLS
retry. Successful policy is retained privately with pinned metadata for package
download; logs record only its name, never proxy endpoints or credentials.

Platform TLS is preferred; certificate-related failures retry once with locked
Mozilla roots on the same policy and deadline. Hostname/certificate checks and
TLS 1.2 minimum remain. Legacy protocol, status, redirect, bounds and integrity
errors do not trigger root retry. Package download has one 300-second budget and
never silently switches policies on failure. Known owned partial files are retired
before root retry; failure/cancellation cleans pending directories. Existing
private cache, checksum, staging, admission and durable activation/rollback remain.

Managed generation/PID/revision/phase changes or a closed watch cancel network
work, with identity checks again before cache reuse/publication. Shutdown cancels
the entire chain; network remains outside the actor. Subscription downloads reuse
the managed route/parser/watch guard and existing private environment/TLS helpers,
preserving explicit direct/self_proxy/with_proxy semantics.

Verification: `cargo check --workspace --locked --offline`, 249 regular Rust
tests, 66 opt-in tests using real Mihomo v1.19.31, 23 production-bundle browser
workflows, Clippy with warnings denied, formatting and diff checks pass. New
coverage exercises private managed proxy authentication, retained package routing,
metadata fallback, package HTTP/integrity failures without policy switching,
generation/revision/stop/closed-watch cancellation and pending-file cleanup,
process-isolated environment/NO_PROXY/CGI behavior, and rejection of untrusted or
wrong-host TLS certificates after both verified-root attempts. Existing subscription
proxy/TLS/cancellation and activation/recovery regressions remain green.

The fresh `target/mihomo-server-linux-x86_64-core-network` Linux bundle passes
checksum verification; its service binary matches the fresh release build and its
deployment/provenance documents match the sources. Official v1.19.31 discovery,
same-version no-op, forced installation of the 22,805,792-byte package, stopped
state retention, running PID/inode replacement and durable receipt readback after
service restart pass. A private copy of the actual 56-node subscription verifies
managed-route discovery, verified static-root fallback with an unrelated platform
CA, restored node selection and HTTPS 204 proxy traffic before and after official
forced activation. System-proxy subscription HTTPS also reaches YAML validation.
Original data hashes are unchanged. Temporary services, cores, script workers and
probe processes are terminated/reaped; the final process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done. Next Delivery
step 7 subtask: durable repair of an unreadable/empty managed core, including failed
repair rollback/recovery and browser repair readback. Alpha/other targets, native
TUN/DNS/hosts/resources, backups/WebDAV, advanced pages, garbage collection,
SOCKS/PAC and platform/deployment work remain pending. Native macOS system
proxy discovery still requires platform runtime checks (Windows compatibility deferred).
<!-- Windows compatibility deferred: Native Windows system proxy discovery still requires platform runtime checks. -->

## Previous increment: durable repair of broken managed cores

Delivery step 7 now permits management startup with an empty, unreadable or
nonexecutable existing managed core, without replacing it with the bundle seed.
Only bounded, owner-owned, unshared regular files with safe ordinary permissions
are admitted; links, unsafe ownership/permissions and capability-bearing upgrades
remain rejected. A missing owner read/execute permission, empty file or failed
bounded version probe returns the public installed version `unknown`. Shutdown
and file-safety failures remain errors. Unknown versions never take the same-version
no-op path, so the stable wrapper and explicit staging/activation can repair them.

Normal upgrades keep schema-1 digest-backed rollback. Repairs use schema-2 journals
with fixed file identity (device/inode, size, mtime and mode) and a hard link to the
original inode inside the private transaction directory. Backup does not read or
chmod the old file. Candidate publication is one atomic rename after fresh version,
configuration and digest validation. Pending rollback/restart recovery restores the
exact original inode and permissions; candidate/backup identity conflicts fail
closed. Interrupted rollback is idempotent. Committed recovery verifies the new
bytes, writes the new receipt and retires the backup. Existing schema-1 journals
remain readable. A valid old receipt whose bytes no longer match the broken file
is preserved during failed repair; malformed/unsafe receipts still require recovery.

The browser reads installed version and receipt independently. Failed receipt
verification remains visible while an admitted unknown version enables repair.
It shows `未知（需要修复）`, an unverified-record state and a repair result, then
rereads the verified version/receipt after completion and reconnect. Failed repair
retains the previous file and allows retry; a previously stopped/failed core stays
stopped after a successful repair until explicitly started. Saved profiles, runtime
configuration and node records are retained.

Verification: `cargo check --workspace --locked --offline`, 253 regular workspace
Rust tests, the 67-case real-Mihomo opt-in workspace suite and targeted real
execute-only/SIGKILL repair regressions pass (68 distinct opt-in cases in total).
Formatting, Clippy with warnings denied, TypeScript/Vite production build and diff
checks pass. Tests cover empty/unreadable/nonexecutable original files, pending and
committed boundaries, a crash after rollback rename, old receipt preservation,
unsafe/shared/privileged files, changed inode/backup/live identity, malformed records
and dangling receipt links rejected before any switch. Real Mihomo verifies
unreadable and execute-only files, stopped-state retention, unchanged configuration/
profiles, restored saved node selections and durable receipt readback. SIGKILL
terminates/reaps the candidate, and next startup restores the exact broken inode;
management remains available and a subsequent valid repair succeeds. Existing
normal-upgrade SIGKILL recovery also remains green.

All 24 browser workflows pass with the final production bundle, including partial
version/receipt readback, a real failed repair preserving empty-file inode/mode,
retry with actual staging/activation, verified receipt and restart readback. A
private copy of the actual 56-node subscription completes the official latest-stable
repair with `force: false` and `from: "unknown"`, preserves stopped state and the
configuration/catalog, restores its selected node after starting/restarting, and
returns HTTPS 204 through the proxy before and after repair. Verified static-root
TLS retry succeeds with an unrelated platform CA. Original data hashes are unchanged.

The runnable `target/mihomo-server-linux-x86_64-core-repair-final` bundle passes all
checksums; its service binary matches the fresh release build and its deployment/
provenance documents match the sources. Temporary services, cores, script workers
and probe processes are terminated/reaped; the final process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done. Next Delivery
step 7 subtask: Alpha release metadata/preparation with bounded version and asset
validation, followed by Alpha activation/rollback and browser integration. Other
targets, native TUN/DNS/hosts/resources, backups/WebDAV, advanced pages, garbage
collection, SOCKS/PAC and platform/deployment work remain pending.

## Previous increment: Alpha metadata and compressed candidate preparation

Delivery step 7 adds authenticated `alpha_core_release` and
`prepare_alpha_core_upgrade`, each with an optional bounded Alpha version.
Discovery requests the official `Prerelease-Alpha` API release and requires its
published prerelease status. The unique ordinary Linux amd64-v2 gzip asset supplies
`alpha-<7..40 lowercase hex>` version, declared size and GitHub SHA-256 from one
metadata snapshot. Other Go/platform variants are ignored; missing, ambiguous,
invalid, draft or mismatched assets/URLs/digests are rejected. Requested versions
must equal the currently published Alpha asset. Arbitrary URLs/hashes/paths remain
unsupported; stable commands still reject Alpha requests and prerelease metadata.

Alpha package URLs use the fixed `Prerelease-Alpha` tag and exact resolved asset
name. The selected managed/system/direct route and verified platform/static TLS
policy remain pinned across download. Existing 1 MiB metadata, 64 MiB package,
timeout/admission/shutdown, gzip-signature and SHA-256 checks apply. A moving release
cannot silently replace the resolved version/hash: missing or changed bytes fail
without publishing a partial candidate. IDs use `alpha-<commit>-<package digest>`;
readback splits the final digest separator, checks fixed manifest identity and
rehashes cached bytes after restart. Existing stable IDs/manifests remain compatible.

This increment prepares compressed candidates only. It does not execute or activate
Alpha files. `stage_core_upgrade` rejects Alpha before unpacking/probing; Alpha
staged receipts, durable activation/rollback, force/no-op and browser controls remain
pending. The live core, configuration and profile/node records remain authoritative.

Verification: `cargo check --workspace --locked --offline`, 257 regular workspace
Rust tests, all 68 opt-in tests using real Mihomo, Clippy with warnings denied,
formatting and diff checks pass. New fixtures cover default variant selection,
channel/version rejection before network, published Alpha tag/status, invalid or
ambiguous assets, digest/size/URL/metadata bounds, a moving metadata snapshot,
package integrity failure/cancellation, restart readback and the pre-execution
staging gate. Management tests verify authentication, source-override rejection and
bundle requirements for the new commands. Existing stable activation/repair,
crash rollback, subscription routing/TLS and configuration regressions remain green.

All 24 browser regression workflows pass with the fresh Linux bundle. A private
copy of the actual 56-node subscription discovers/prepares official
`alpha-63bd52e` (22,849,242 compressed bytes) through its managed proxy, with
verified static roots under an unrelated platform CA. Package size and SHA-256
match official metadata; restart cache readback is identical. Alpha executable
staging returns an explicit error, and preparation preserves live PID/inode,
installed stable version, runtime YAML, profile catalog and selected node. Proxy
HTTPS returns 204 before and after preparation. Original data hashes are unchanged.

The runnable `target/mihomo-server-linux-x86_64-alpha-preparation` bundle passes
checksums; its service binary matches the fresh release build and its deployment/
provenance documents match the sources. Temporary services, cores, script workers
and probe processes are terminated/reaped; the final process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done. Next Delivery
step 7 subtask: Alpha executable/version/config staging, then durable activation,
rollback/installation receipts and channel-aware force/no-op/browser integration.
Other targets, native TUN/DNS/hosts/resources, backups/WebDAV, advanced pages,
garbage collection, SOCKS/PAC and platform/deployment work remain pending.

## Previous increment: Alpha executable, version and configuration staging

Delivery step 7 now accepts an Alpha prepared ID in `stage_core_upgrade`. Stable
and Alpha share the bounded, private gzip/ELF extraction pipeline and actor-owned
configuration snapshot. Stage ID parsing splits package/config digests from the
right so hyphenated Alpha versions remain intact; existing stable IDs and schema-1
stage manifests stay compatible. `staged_core_upgrade` rechecks the prepared
package, manifest, executable size/hash/mode and candidate YAML hash after restart.

Every stage requires one gzip member with valid CRC/EOF and no trailing data,
a Linux x86_64 ELF, at most 128 MiB unpacked bytes and a 15-second unpack budget.
The candidate reports the exact resolved Alpha version in a bounded five-second
`-v` probe, then validates current YAML using `-t` with its own private resource
directory. Both streams remain bounded to 64 KiB; shutdown cancels/reaps probes.
Configuration/resource limits remain 8 MiB/256 MiB. Executable and YAML hashes are
checked again after execution, before fsync/atomic publication. Repeated staging
runs probes again before returning an existing immutable proof. Failure removes
only pending work and keeps the compressed candidate for retry.

The live core, profile catalog, runtime revision, configuration and node selections
are not replaced. Alpha activation is explicitly rejected immediately after staged
readback, before fresh probes, journaling, stopping or file switching. Durable Alpha
activation/rollback/receipts, force/no-op and browser controls remain pending.

Verification: `cargo check --workspace --locked --offline`, 261 regular workspace
Rust tests, all 68 opt-in tests using real Mihomo, Clippy with warnings denied,
formatting and diff checks pass. New Alpha fixtures cover exact version/config
proofs, private Geo validation resources, cache revalidation/restart readback,
wrong version, configuration rejection, executable/YAML mutation, both probe-phase
cancellation/reaping and pending cleanup. Stable and Alpha readback tests reject
unsafe IDs, config tampering, executable modes and links. Actor tests confirm that
Alpha activation is gated without additional probes, status changes, receipts or
switch journals. Existing stable activation/repair/crash and subscription regressions
remain green.

All 24 browser workflows pass with the fresh production bundle. A private copy of
the actual 56-node subscription prepares official `alpha-63bd52e` (22,849,242
compressed bytes), stages the real executable against the current generated YAML,
revalidates an identical cached proof and reads it back after service restart.
Managed routing and verified static-root TLS retry under an unrelated platform CA
remain functional. A wrong Alpha version and tampered executable/YAML readback are
rejected. Activation returns its explicit pending-integration error without creating
a journal. The running stable core's PID/inode/version, config, catalog and selected
node are preserved; proxy HTTPS returns 204 before and after staging. Original data
hashes are unchanged.

The runnable `target/mihomo-server-linux-x86_64-alpha-staging` bundle passes all
checksums; its service binary matches the fresh release build and its deployment/
provenance documents match the sources. Temporary services, cores, script workers
and probe processes are terminated/reaped; the final process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done. Next Delivery
step 7 subtask: durable Alpha activation/rollback and installation-receipt recovery,
then channel-aware force/no-op and browser integration. Other targets, native
TUN/DNS/hosts/resources, backups/WebDAV, advanced pages, garbage collection,
SOCKS/PAC and platform/deployment work remain pending.

## Previous increment: durable Alpha activation, rollback and receipt recovery

Delivery step 7 now admits verified Alpha proofs through authenticated
`activate_core_upgrade`. Stable and Alpha share the existing actor transaction:
current-YAML digest check, fresh bounded version/configuration staging, previous
installation snapshot, atomic replacement, runtime readiness, exact live version
and proxy-port checks, durable commit, receipt publication and node restoration.
A stopped core stays stopped after validation; a running core resumes its saved
profile/node selection. Runtime configuration and catalog are not rewritten.

Installation stage IDs split both package/configuration digests from the right
and use the same bounded stable/Alpha version grammar as release preparation.
Existing stable schema-1 upgrade journals/receipts and schema-2 repair journals
remain compatible; no schema bump, channel flag or dependency is introduced.
Malformed Alpha commits, mismatched versions/config digests, invalid package
hashes and unsafe record paths remain fail-closed before recovery overwrites.

Pending switches restore the exact previous executable and receipt, including
stable-to-Alpha, Alpha-to-stable and Alpha-to-Alpha transitions. Committed recovery
retains the verified replacement and publishes its receipt. Repair can snapshot
an unverified previous Alpha receipt without reading/chmodding a broken core;
failure restores the old inode/mode/receipt, while commit publishes the validated
replacement. Existing shutdown cancellation, Linux parent-death termination,
owned-process reaping and conflict-preserving recovery remain authoritative.

An isolated copy of the actual 56-node subscription prepares official
`alpha-63bd52e` (22,849,242 compressed bytes), stages and activates its exact
executable/current YAML, switches Alpha back to stable, activates Alpha while
stopped and starts it explicitly. Running transitions change PID/inode and keep
configuration/catalog/node selections; each transition and service restart passes
HTTPS proxy traffic with status 204. Failed stable and Alpha runtime fixtures
restore the previous real Alpha executable and exact installation receipt.
Restart retains that receipt and selected node without overwriting the active
Alpha with the stable bundle seed. Wrong Alpha versions and tampered proof
readback are rejected. Managed routing and verified static-root TLS retry under
an unrelated platform CA still work. Original data hashes remain unchanged.

All 24 browser workflows pass with the fresh production bundle. Its service
binary matches the release build, deployment/provenance documents match their
sources and every bundle checksum passes. Existing stable upgrade and broken-core
repair workflows remain available; Alpha browser controls are still pending.

Verification: `cargo check --workspace --locked --offline`, all 264 regular
workspace Rust tests, all 71 opt-in tests with real Mihomo, Clippy with warnings
denied, formatting and diff checks pass. Alpha tests cover fresh revalidation,
failed stopped/running readiness, stale config rejection, shutdown rollback,
SIGKILL during normal activation/repair, orphan termination and startup recovery.
Receipt/journal tests cover both transition directions and Alpha-to-Alpha,
7/40-character commit IDs, pending/committed boundaries, broken Alpha receipt
repair and malformed versions/hash/identity records without overwrites. The real
repair regression now waits at most five seconds for proxy-group publication and
saved-node restoration instead of assuming that the first controller-ready
snapshot contains the group; its focused test and complete opt-in run pass.

The runnable `target/mihomo-server-linux-x86_64-alpha-activation` bundle retains
the verified stable bootstrap core and supports independently installed Alpha.
Temporary services, cores, script workers, validators and actual-data probes are
terminated/reaped; the final process audit reports zero owned fixture processes.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the complete project is not done. Next Delivery
step 7 subtask: channel-aware Alpha force/no-op orchestration and browser controls,
including reconnect/receipt readback and repair. Other targets, native
TUN/DNS/hosts/resources, backups/WebDAV, advanced pages, garbage collection,
SOCKS/PAC and platform/deployment work remain pending.

## Previous increment: Alpha force/no-op orchestration and channel-aware Web controls

Delivery step 7 adds authenticated `upgrade_alpha_core` with a required boolean
`force`. Its stable counterpart, `upgrade_clash_core`, keeps its existing request
shape and stable behavior. Both dispatch one shared channel-aware pipeline:
exclusive upgrade admission, official channel metadata discovery, actor-owned
installed-version no-op check, verified preparation, a second no-op check after
the download, current configuration staging, durable activation/receipt recovery.
The request cannot supply a version, URL, digest, path or arbitrary channel.
Disconnect does not cancel an accepted switch; shutdown cancels/reaps owned work.

Default Alpha upgrades skip the same version before package/stage access and
preserve live PID/inode/configuration/receipt. Force bypasses both no-op checks
and revalidates/reinstalls the resolved Alpha. Unknown installed versions never
skip; the existing inode-preserving repair transaction handles broken cores.
A different stable/Alpha version is a real switch regardless of the current
channel. Actor rechecks retain lifecycle/configuration changes made during download.
No channel flag or journal schema change is introduced.

`/core` now selects stable or Alpha and issues channel-specific discovery/upgrade
commands. Stable is the default on a new page session; selection is per operation,
not a persistent core-setting override. Installed version/receipt are always read
from the actual managed file, including after reconnect and service restart.
Changing channel clears previous release/report output; in-flight operations lock
channel controls. Force confirmation names the chosen channel. Alpha pre-release
status, stable switch-back, unknown-version repair and failure/retry remain visible.

Verification: `cargo check --workspace --locked --offline`, all 265 regular
workspace Rust tests, all 71 real-Mihomo opt-in tests, warnings-denied Clippy,
formatting/diff checks and the TypeScript/Vite production build pass. Alpha no-op
fixtures deliberately remove package data and confirm no stage/state/file/receipt
changes; force enters validation, fails readiness and restores the original core.
Authentication, missing/wrong force types and arbitrary version/source/channel
fields remain rejected before mutation. Existing lifecycle, Alpha crash/repair,
subscription, settings and receipt regressions remain green.

All 26 browser workflows pass, including both channel variants using a verified
real Alpha executable. They check channel-specific requests, locked controls,
cleared stale results, force-confirmation dismissal/acceptance, failed retries,
running/stopped reinstall, real receipt readback after restart and broken-core
repair preserving the original inode/mode on failure. Browser metadata/no-op
results are deterministic fixtures; staging/activation/rollback/readback exercise
the real service. Official-wrapper discovery/preparation/no-op/force/repair are
separately exercised with real-node traffic rather than browser interception.

An isolated copy of the actual 56-node subscription resolves official
`alpha-63bd52e` (22,849,242 compressed bytes) and calls `upgrade_alpha_core`.
Default equal-version no-op preserves PID/inode/receipt; force reinstalls with a
new PID/inode. Alpha-to-stable and stopped stable-to-Alpha work; an empty mode-0
core reports unknown and is repaired to Alpha without force. Failed stable/Alpha
runtime candidates restore the real Alpha receipt. Restart keeps that receipt,
configuration/catalog and selected node; HTTPS traffic returns 204 after each
transition. Managed routing/static trusted roots under an unrelated platform CA
still work. Wrong versions/tampered proofs are rejected; original data hashes stay
unchanged. No fixture processes remain after termination/reaping.

The runnable `target/mihomo-server-linux-x86_64-alpha-controls-final` bundle
passes every checksum. Its service matches the release build, Web assets match
the tested production bundle and deployment/provenance documents match sources.
It retains a verified stable bootstrap seed and supports independently installed
Alpha. The temporary verified Alpha browser fixture is removed after checking.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the full project is not complete. Next Delivery
step 7 subtask: extract backup models and implement bounded local backup export
with an authenticated API, before restore transactions, scheduling and WebDAV/UI.
Other upgrade targets, native TUN/DNS/hosts/resources, advanced pages, garbage
collection, SOCKS/PAC and platform/deployment work remain pending.

## Latest increment: bounded local backup export and portable metadata

Delivery step 7 adapts upstream `create_backup` ZIP/configuration export and local
filename/length metadata into service-owned backup models. `headless-core::backup`
defines schema-1 manifest, entry and download metadata with strict fields, safe
relative paths, SHA-256 digests, required configuration entries and size/count
validation. Host paths are not returned; the format is identified as
`mihomo-server`, not a desktop restore bundle. Stored ZIP entries retain upstream's
uncompressed container choice and profile raw bytes.

Authenticated `POST /api/backup` accepts an empty body only and returns
`application/zip`, an attachment filename, Content-Length and X-Backup-SHA256.
The existing bearer/Host/Origin/query controls and no-store/nosniff headers apply.
The actor finishes pending transaction recovery before fixing profiles, settings,
active profile/runtime revision and source configuration. Its awaited blocking
worker excludes competing actor writes while building a consistent snapshot;
core proxy traffic keeps running. A download-owned semaphore permit admits one
queued/building/streaming export and is released at EOF or disconnect. Failed
source checks expose a generic HTTP error instead of host paths or file contents.

The archive contains `manifest.json`, serialized `profiles.yaml` and
`settings.yaml`, controller-boundary-validated `runtime.yaml` and catalog-referenced
raw subscription/global/linked enhancement files under `profiles/`. Node records,
source credentials/comments and enhancement contents are preserved as backup
content. Management tokens, locks, live controller files/sockets, transaction
journals, orphan profiles, binaries, Geo/provider caches and upgrades are excluded.
Typed settings exclude desktop WebDAV credentials. No archive is persisted in the
service data directory and no request controls a source/destination path.

Bounds: 1,024 ZIP entries including the manifest, 8 MiB per content entry,
64 MiB content and 65 MiB archive output. A 15-second cooperative worker budget
and shutdown checks apply between bounded reads and ZIP writes; regular filesystem
I/O cannot be forcibly preempted. Shutdown joins the worker before directory
ownership is released. Unix descriptor-relative/no-follow reads pin the profile
directory; unsafe ownership/write modes, links, shared files, nonregular files,
oversized files and changed identities are rejected. Archive entries have mode
0600. Export leaves lifecycle/configuration/catalog/node selection unchanged.

Verification: `cargo check --workspace --locked --offline`, all 272 regular
workspace Rust tests, all 72 real-Mihomo opt-in tests and the focused real backup
regression pass. Warnings-denied Clippy, formatting and diff checks pass. New tests
cover portable manifest roundtrip/strict fields/path/hash/required-file/count/size
bounds, raw/selection preservation, ZIP mode/CRC/digests, links/hard links/FIFO/
directories/unsafe permissions, sparse oversized/aggregate files, cancellation,
invalid runtime controllers, HTTP auth/origin/method/query/body rejection,
sanitized failures and the body-owned permit released on disconnect or EOF.
Real-core snapshots preserve running PID/generation/config/records, stopped state
and persisted readback after manager restart. Existing Alpha, subscription,
lifecycle and settings regressions remain green.

All 24 default browser workflows pass with the fresh production bundle; the two
optional Alpha executable browser cases are explicitly skipped without their
separate verified-binary fixture. Web source/assets are unchanged in this backend
increment. Alpha activation/repair/crash behaviors remain covered by the 72
real-core tests. The package passes every checksum; service binary matches the
release build and deployment/provenance documents match their sources.

An isolated copy of the actual 56-node subscription exports a 1,110,883-byte ZIP
with 13 entries. Python ZIP CRC validation and every manifest length/SHA-256 pass;
raw profiles/auxiliaries match the copied source bytes, runtime/settings/catalog
match authenticated readback, and management credentials/controller/core files
are absent. Running PID/generation/config/catalog/selected node remain unchanged;
stopped and restarted service exports also pass. HTTPS proxy traffic returns 204
before/after export and after restart. Original data hashes remain unchanged.
The runnable `target/mihomo-server-linux-x86_64-backup-export` bundle keeps the
working stable bootstrap and existing stable/Alpha upgrade workflows. Temporary
services, cores, script workers, validators and real-data probes are terminated/
reaped; the final owned-process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the full project is not complete. Next Delivery
step 7 subtask: strict backup archive inspection/validation before implementing
transactional restore and rollback. Local archive retention/list/delete, automatic
backup scheduling, WebDAV and backup UI remain pending, together with other targets,
native resources/settings, advanced pages, garbage collection and platform work.

## Latest increment: strict read-only backup inspection

Delivery step 7 now implements the validation phase ahead of service backup
restoration. `headless-core::backup::BackupInspection` returns bounded format/time/
count/digest metadata without source contents, names, URLs or host paths. Existing
sequence validation is exposed for reuse; source subscription controller removal
and script-source bounds retain the existing processing semantics.

Authenticated `POST /api/backup/inspect` accepts an unencoded `application/zip`
body and returns a JSON report. The existing bearer/Host/Origin/query boundary,
no-store and nosniff headers apply. Before buffering any upload, it obtains the
same single permit used by export generation/downloads; concurrent inspection
returns 409. Failed uploads, timeout and disconnect release admission; a worker
already running retains the permit until completion/cancellation. Inspection
does not use the lifecycle actor queue, so status/settings/lifecycle operations
continue independently. No files are extracted or retained, no uploaded scripts
run, no Mihomo validation process starts and running state is not modified.

Strict Stored ZIP32 preflight scans central records before library name indexing
can hide duplicate paths. Every local/directory name, flag, timestamp, CRC and
length must match; ranges must cover contiguous local records and the exact
directory/footer. Safe relative UTF-8 names and private regular-file modes are
required. Prefix/trailer bytes, comments/extras, overlaps, links/directories,
encrypted/compressed entries, data descriptors, ZIP64 and multi-disk variants
fail closed. The pinned zip reader checks CRC without allocating entry contents;
manifest paths/counts/lengths/SHA-256 must cover every content entry exactly.

Configuration validation checks strict catalog fields, unique bounded UIDs,
known/reserved types, referenced profile files, linked enhancement types, current/
active-profile coherence, settings profile references and service controller
ownership. Raw source/merge/sequence YAML and UTF-8 script bounds use existing
validators; shared same-type files are validated once. Success establishes
archive integrity and configuration coherence, not JavaScript/Mihomo execution
or transactional restore readiness.

Limits: 65 MiB archive/upload, 64 MiB content, 8 MiB per content entry, 1,024 ZIP
entries, 1 MiB manifest and the existing 64 KiB settings bound. The dedicated
binary route accepts a valid 10 MiB archive while JSON commands retain their
9 MiB envelope bound. Upload timeout is 15 seconds; worker processing has a
separate 15-second cooperative budget and HTTP/manager shutdown checks. YAML
parsing cannot be forcibly preempted. Errors are generic and exclude uploaded
contents and host paths; request/media/size/shutdown boundaries have distinct
HTTP statuses. Neither restore destination nor arbitrary source path is accepted.

Verification: `cargo check --workspace --locked --offline`, all 281 regular
workspace tests with `--test-threads=1`, all 72 real-Mihomo opt-in tests, focused
inspection regressions, warnings-denied Clippy, formatting and diff checks pass.
The initial parallel workspace run hit two existing core-upgrade unit assertion
failures; both isolated reruns and the full serialized run pass. Their cause is
not established. New coverage includes malformed/duplicate/ambiguous ZIP records,
CRC/manifest tampering, valid hashes with broken domain references, Unicode and
all linked types, cancellation/deadline, export compatibility, pre-body admission,
disconnect/shutdown release, chunked size limits and separate binary/JSON limits.
Real-core tests inspect running/stopped/restarted exports while preserving PID,
generation, configuration, catalog and saved node selection.

An isolated copy of the actual 56-node subscription exports a 1,110,883-byte,
13-entry archive and passes authenticated inspection while running, stopped and
after service restart. Python independently verifies CRC/length/SHA-256/raw bytes;
trailing data and a freshly hashed but inconsistent current-profile catalog are
rejected without state changes. Selected-node HTTPS proxy requests return 204
before/after inspection and after restart; original data hashes are unchanged.
The fresh runnable `target/mihomo-server-linux-x86_64-backup-inspection` bundle has
12 valid checksums; its service binary matches the release build and bundled
deployment/provenance documents match source. Web source/assets are unchanged.

All 24 default browser workflows pass against the fresh production bundle; two
optional Alpha executable cases are skipped without their separate verified
binary fixture. Stable/Alpha activation/repair/recovery remain covered by the
72 real-Mihomo tests. Isolated services, cores, workers and validators are
terminated/reaped; the final owned-process audit reports zero.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the full project is not complete. Next Delivery
step 7 subtask: transactional backup restore with candidate runtime validation,
durable publication/recovery and failure rollback. Local retention/list/delete,
automatic schedules, WebDAV and backup UI remain pending, along with other core/
release targets, native resources/settings, advanced pages, garbage collection,
shared optional components and platform/service integrations.

## Latest increment: disposable restore candidate and runtime validation

Delivery step 7's restore work is split into candidate validation and durable
publication. This increment completes the functional candidate rehearsal phase:
`BackupRestoreValidation` models and authenticated `POST /api/backup/validate`
check an uploaded backup against the current core without publishing settings,
catalog or runtime state. Durable restoration, recovery and rollback are still
pending; the report is not an installation receipt or reusable staged candidate.

The route reuses inspection's strict binary upload boundary and shared backup
admission before buffering, then sends validation to the lifecycle actor after
normal pending-transaction recovery. Actor serialization keeps the core stable
against concurrent upgrades/configuration/lifecycle commands. Existing proxy
traffic and watched state continue; queued actor commands wait. A disconnected
reply, HTTP closing or manager shutdown triggers a private cancellation channel.
The actor joins preparation, terminates/reaps probes/workers and removes the
candidate before releasing admission and data-directory ownership.

Strict archive verification now also provides borrowed verified manifest/entry
contents for internal consumers. The disposable worker materializes only verified
configuration/profile paths into a fresh 0700 temporary directory with 0600 files,
preserving raw source bytes and excluding credentials/runtime sockets/core files.
Existing ProfileStore generation reads the isolated catalog/files. Current Geo
resources are copied into separate validation data with no-follow/nonblocking
reads, owner/mode/type/link/identity checks and a combined 256 MiB limit. Provider
paths must be relative and remain within disposable validation data; omitted
provider caches can fail probes. Mihomo can fetch resources into that disposable
directory; current-core resource isolation is not a full OS sandbox.

The exact archived runtime is checked first with current Mihomo `-t`. If there is
an active profile, its raw/sequence/global/profile source and archived settings
produce a separate regenerated candidate following existing settings/TUN/DNS,
merge/script/final-authority/finalization order without deriving twice. Identity
scripts skip processes; other active scripts use the existing bounded Linux Boa
worker. Uploaded logs and diagnostics are discarded rather than appended to live
logs. Inactive profiles receive source/schema checks, not execution/core probes.
Requested provider DNS overrides need fresh session confirmation and are suppressed
in regeneration until then, with a boolean reported to the caller. No live DNS
confirmation or preference changes occur.

Responses contain the inspection metadata and separate archived/regenerated byte
lengths/SHA-256, plus the DNS confirmation flag; they expose no YAML/source/profile
names, paths or logs. Digests may differ for manual runtime edits or session DNS
policy. Both runtime candidates are checked for bounded bytes and unchanged
regular-file identity/content after probes. No runtime revision, restore journal,
catalog/settings publication or candidate proof is retained. Normal success,
failure and cancellation clean temporary data; cleanup errors fail the request.
Abrupt process termination can leave private temporary files; orphan cleanup is
an explicit pending restore recovery responsibility.

Upload bounds remain 65 MiB/15 seconds and one shared backup slot; preparation has
a separate 15-second cooperative filesystem budget. Script/core probes use existing
configured timeouts (5 seconds by default), IPC/output limits and cancellation.
Parsing/filesystem I/O cannot be forcibly preempted. Generic 422 failures cover
archives, regeneration/scripts, missing/unsafe resources, Mihomo rejection,
candidate mutation and cleanup, without leaking uploaded content or host paths.

Verification: `cargo check --workspace --locked --offline`, all 288 regular Rust
tests with `--test-threads=1`, focused backup/restore tests, warnings-denied Clippy,
formatting and diff checks pass. New tests cover private staging/raw preservation/
cleanup, Geo links/hard links/FIFO/unsafe modes/oversize, bounded no-follow candidate
checks, provider traversal, bootstrap-only and active rehearsals, script execution/
failure/loop timeout, preserved live settings/state/logs, archived authority and
fresh provider DNS confirmation. Disconnect/HTTP closing/manager shutdown tests
observe probe PIDs and verify reaping and temporary-directory removal. Real-core
backup regression also rehearses running/stopped/restarted archives and rejects
format-valid but unsupported-protocol runtime while preserving the live core.

An isolated copy of the actual 56-node subscription exports a 1,110,883-byte,
13-entry archive. Both archived runtime and active regeneration pass rehearsal
while running, stopped and after service restart. Running PID/generation/config/
catalog/selected node remain unchanged. Selected-node HTTPS proxy requests return
204 before/after rehearsal and after restart, and original data hashes are
unchanged. The fresh runnable Linux bundle is
`target/mihomo-server-linux-x86_64-restore-validation`; all 12 checksums pass, the
binary matches the release build and bundled deployment/provenance docs match
source. Web source/assets are unchanged.

All 72 real-Mihomo opt-in tests and all 24 default browser workflows pass against
the fresh production bundle. Two optional Alpha executable browser cases are
skipped without their separate verified-binary fixture; stable/Alpha core paths
remain covered by the real-core regressions. The final audit reports zero owned
service/core/worker/validator processes and zero remaining restore candidate
directories after normal success, failures and cancellation.

Git handoff: no sandbox Git writes/commits; the external host script owns the commit.
The Linux MVP remains runnable; the full project is not complete. Next Delivery
step 7 subtask: durable backup restoration coordinating catalog/settings/runtime
publication with startup recovery and failure rollback, including temporary
candidate ownership/cleanup. Retention/list/delete, schedules, WebDAV, backup UI,
other targets, native settings/resources, advanced pages, garbage collection,
optional shared components and platform/service integrations remain pending.

## Latest increment: durable backup restore intent and startup recovery

Delivery step 7 now has a working persistence layer for backup restoration.
`ProfileStore::prepare_restore` returns an opaque bounded plan from an already
verified disposable catalog and a staged runtime revision. `begin_restore`,
`publish_restore` and `recover_restore` coordinate catalog, settings and source
files through a single private `backup-restore.yaml` journal. The service uses
recovery at startup and before actor commands. Online restoration is still
pending: no upload route invokes publication yet, and rehearsal reports remain
read-only metadata rather than installable receipts.

Preparation checks matching store roots, settled runtime/catalog identity, source
schemas, UID/type/active/link/DNS references, source file boundaries and existing
transaction admission. It preserves raw bytes, UID/metadata/auxiliary links and
node records, allocating new immutable filenames for unique archived sources.
Shared files of the same type remain shared; different types cannot share one
source. Old live files are retained untouched. New sources are limited to 1,020
files, 8 MiB each (1 MiB for scripts), 64 MiB combined; catalogs remain 8 MiB and
settings 64 KiB. The bounded journal holds previous/candidate pointers and settings,
source lengths/digests and the staged runtime digest, with an 18 MiB limit.

The caller validates generation/scripts/Mihomo and decides runtime/DNS publication
policy before registering the candidate with `RuntimeStore::begin_profile`.
Preparation's source parsing is not equivalent to a core probe and does not
execute scripts. Durable journal intent precedes any allocated source write.
Each source uses a private `.part` file, fsync, rename and profiles-directory sync.
Catalog/settings publication precedes `RuntimeStore::commit`; that manifest rename
is the sole logical commit marker, even if its subsequent directory fsync fails.
A caller must distinguish an already committed manifest from a precommit error.

After restart RuntimeStore first discards pending activation. Recovery compares
committed revision and active UID: the previous revision rolls catalog/settings
back and removes only allocated sources/partial files; the candidate revision
finishes publication and retains restored sources. Cleanup is idempotent and the
journal is removed only after pointers/files are consistent. A truncated private
`.part` is recoverable; finalized sources must match their recorded digest before
any recovery mutation. Missing committed files, changed sources/runtime, unsafe
files/directories, a third catalog/settings/runtime state or conflicting journals
fail closed and retain intent for investigation. Recovery never overwrites an old
source or rolls a committed revision back. Precommit callers must abort pending
runtime before invoking recovery.

New journal/source reads are bounded and no-follow/nonblocking, checking regular
file type, owner/private permissions, single links and stable file identity.
Directory checks reject symlinks/foreign ownership/unsafe write modes. Pending
restore intent blocks new import/edit/delete/enhancement/default/settings
transactions. Startup resolves it before other configuration journals, DNS
preference pruning, current-profile mirroring and global defaults; actor commands
also stop on unresolved recovery. The implementation and syscall dependency are
Unix-only; Linux is verified and other restore targets remain pending. Filesystem
operations are bounded synchronous work under caller ownership, not a cancellable
online restore worker or an OS sandbox.

Verification: `cargo check --workspace --locked --offline`, 300 regular Rust tests,
11 focused persistence tests (including the additional first-restore case),
the 3 real-Mihomo backup tests, warnings-denied Clippy,
formatting and diff checks pass. The recovery matrix covers partial source staging,
files without publication, catalog-only/settings-only/both publication, committed
pointer repair, repeated recovery, shared and empty catalogs, stale plans,
first restore without a committed revision, root/runtime conflicts,
source/runtime mutation, missing files, unsafe parent
symlinks, hard links, modes, forged paths/sizes/schema and admission conflicts.
Service tests verify startup order, preserved DNS preferences and data-lock release
on recovery failure. Real-core startup verifies restored configuration and saved
node choice through a second restart.

A separate real-core test privately imports the actual 56-node source from `data`,
probes the staged candidate, commits a restore journal, starts the service through
recovery and checks HTTPS 204 proxy traffic. A second service restart retains the
restored source, selected node and HTTPS 204 traffic. Original catalog/raw bytes
remain unchanged; uploaded contents/node names are not printed. These checks
exercise the persistence layer directly, not an unimplemented HTTP restore route.

The runnable Linux MVP is refreshed in
`target/mihomo-server-linux-x86_64-restore-journal`, with the current Rust binary,
existing Web assets, independent pinned Mihomo and current deployment/provenance
docs. All 12 bundle checksums pass; the packaged binary and docs match source.
Fresh-bundle smoke checks export/inspect/rehearse running, stopped and restarted
56-node data, preserving PID/generation/catalog/configuration and selected node.
The archive has 1,110,883 bytes and 13 entries, proxy HTTPS returns 204, and all
original data-file hashes remain unchanged. Web source/assets are unchanged.
No sandbox Git writes/commits occur; the host script owns the commit.

Next Delivery step 7 subtask: connect verified candidate ownership to an
authenticated restore upload and actor operation; define archived-versus-regenerated
runtime and fresh DNS confirmation policy, validate/apply to a running or stopped
core, roll back precommit failures, finish committed cleanup and return a truthful
receipt on disconnect/shutdown/fsync errors. Abrupt-termination disposable candidate
cleanup, retained old-source/revision garbage collection, local retention/list/delete,
schedules, WebDAV and backup UI remain pending alongside the full design's other
platforms, native settings/resources, advanced pages and optional shared components.
The complete project is not finished.

## Latest increment: explicit stopped-core backup restoration API

Delivery step 7 now connects verified upload candidates to the durable restore
transaction. Authenticated `POST /api/backup/restore` accepts a raw ZIP and exactly
one `X-Backup-Runtime: archived` or `regenerated` header. The caller explicitly
chooses runtime policy; missing/unknown/duplicate choices are 400. The route shares
existing bearer/Host/Origin/query controls, exact application/zip upload handling,
65 MiB/15-second bounds and single pre-buffer backup admission. No destination,
server-side archive filename, reusable rehearsal receipt or arbitrary path is
accepted. Archives are not retained. Responses are private, no-store metadata.

The actor checks stopped phase, absence of a child and automatic retry after
configuration recovery and before validation/publication. Running/recovering/
failed or otherwise unsettled cores return 409 with a stop-first instruction.
Lifecycle commands are serialized so a queued start cannot race the check. This
increment deliberately supports stopped restoration; running-core reload/restart
and rollback remain the next bounded integration task. Restoration leaves the
core stopped; an ordinary start later uses the restored snapshot and existing
saved-node reconciliation.

The upload is verified afresh and privately materialized. Existing isolated Geo,
archived-runtime, active regeneration and bounded script/Mihomo probes are reused.
Both archived and regenerated candidates must pass when an active profile exists,
even when archived publication is selected. After probes, all archived source,
catalog/settings/runtime entries are read no-follow, bounded, with owner/private
mode/type/link/identity checks and their original lengths/SHA-256 rechecked before
publication. This closes the probe-induced source mutation gap for rehearsal and
restore. No uploaded diagnostics are appended to service logs or HTTP errors.

`archived` preserves exact runtime bytes, including comments/line endings, using
new validated RuntimeStore::stage_yaml. Settings and catalog sources are restored
from the archive; manual runtime values can intentionally differ from settings.
`regenerated` selects the candidate produced from active raw/global/profile
sources and archived authority, discarding manual runtime-only edits; it requires
an active archived profile. Bootstrap-only archives therefore use archived policy.
Fresh session DNS protection is enforced during generation. Archived policy is
rejected whenever the active raw source has protected provider DNS, because a
snapshot might contain an earlier session's override. Regeneration suppresses
unconfirmed page overrides, persists the active DNS preference disabled when
confirmation was required and reports that requirement. Commit clears all live
session confirmations; subsequent enabling uses the existing confirmation flow.
Inactive preferences remain catalog-bounded and are checked normally on selection.

The actor stages the selected exact bytes, prepares the opaque restore plan,
registers runtime pending, journals new sources, publishes catalog/settings and
commits the runtime manifest. Cancellation is checked before each durable phase.
Preparation disconnect/HTTP-close/manager-shutdown joins filesystem work, cancels
and reaps probes/workers and drops private candidate ownership before releasing
backup admission. Publication is bounded synchronous filesystem work under actor
ownership; it cannot interrupt a single write/fsync. Precommit error/cancellation
restores previous runtime and recovers catalog/settings/files. If recovery fails,
status reports pending recovery and intent is retained; the generic response asks
clients to inspect status rather than promising that nothing changed.

After manifest rename, restoration is logically committed even if directory fsync
fails. The actor does not roll that state back: it finishes journal/private cleanup,
refreshes settings/profile/status watches, clears old retry/selection/session state
and returns a `BackupRestoreReceipt` with committed=true, archive metadata, policy,
exact applied byte length/digest and revision, DNS confirmation flag and
cleanup_pending. Cleanup/durability acknowledgement problems remain observable in
receipt/status. The HTTP route awaits the actor result rather than discarding a
committed receipt on graceful close. A disconnected client can still miss a commit
made after its final precommit check; inspect status/revision before retrying. No
separate permanent installation receipt or exactly-once replay API is promised.

Verification: `cargo check --workspace --locked --offline`, all 306 regular workspace
Rust tests and all 17 regular backup API tests (307 distinct regular tests, including
the additional committed-cleanup case), 3 real-Mihomo backup tests, warnings-denied
Clippy, formatting and diff checks pass. New coverage verifies archived byte
preservation/controller rejection, explicit policy/auth/duplicates, bootstrap and
active restores, regenerated-versus-manual runtime choice, private source mutation,
probe rejection, journaled publication write failure with rollback, committed private
cleanup failure reported without rollback, released shared
admission, fresh DNS protection and service restart persistence. Existing probe
cancellation tests now exercise both rehearsal and restore disconnect/HTTP-close/
manager-shutdown. Real data coverage restores an exported actual 56-node archive
through the API after changing live settings, verifies the running request is 409,
and checks restored mode/source/selected-node HTTPS 204 through restart. Original
catalog and raw subscription bytes remain unchanged.

The runnable Linux MVP is refreshed at
`target/mihomo-server-linux-x86_64-restore-api` using the current Rust binary,
unchanged Web assets, pinned independent Mihomo and updated deployment/provenance
docs. All 12 checksums pass and the packaged release binary/docs match source.
Fresh-bundle smoke verifies a 1,110,883-byte/13-entry actual 56-node archive, running
restore rejection (409), stopped API commit after changing settings, restored
source/settings/node state, start and service-restart HTTPS 204 traffic, and unchanged
original data-file hashes. Runtime/profile/log state remains unchanged during
rehearsal; publication intentionally advances the runtime revision and source
filenames. Final cleanup leaves no owned service/core processes or disposable
restore directories. No sandbox Git writes/commits occur; the host script owns the
commit.

Next Delivery step 7 subtask: extend explicit restoration to running cores with
validated reload, restart fallback, cancellation-aware rollback of the old core,
commit-aware completion and preserved node records. Abrupt-termination private
candidate cleanup, source/revision garbage collection, retained archive list/delete,
scheduled backups, WebDAV, backup UI and the full design's other platform/settings/
resource/advanced-page/shared-component work remain pending. The full project is
not complete.

## Latest increment: running-core backup restoration and rollback

Delivery step 7 extends the explicit backup restoration API to settled running
cores. Stopped behavior remains available. The actor observes child exit and
rejects failed/recovering or otherwise unsettled phases, child/phase disagreement
and scheduled recovery with 409 `restore_requires_settled_core`. Lifecycle and
restore operations remain serialized after journal recovery. The same verified
upload, explicit archived/regenerated policy, isolated validation, fresh DNS
protection, shared admission and journal transaction are reused.

After staging runtime and journaling source files, running restoration cancels
old selection reconciliation and tries reload with existing live proxy-port
readback. A successful reload preserves the PID. Reload failure or a listener
mismatch stops/reaps the old child and starts the candidate, with bounded readiness
and live port verification. Catalog/settings publication and runtime-manifest
commit follow only after successful core application. Receipt now includes
`core_running` and `core_restarted` in addition to committed revision/digests,
policy, DNS confirmation and cleanup state. A stopped restore reports both false.

Core I/O receives a private cancellation watch. Disconnect, HTTP close or manager
shutdown signals it and joins the operation before rollback; stop/start futures
owning children are never dropped. The original manager shutdown watch is then
restored. Precommit failure recovers the previous runtime/catalog/settings/files,
stops/reaps any attempted candidate and restarts the old core with its previous
node records. Failed disk/core recovery exposes Failed/pending state and retains
unresolved intent. Manager shutdown restores disk state and reaps without
restarting. Failure before live application leaves the original child untouched.
A failed/cancelled live apply can change the old core's PID during recovery.

Commit remains the runtime-manifest rename. Postcommit cleanup failure reports
cleanup_pending and does not undo publication. Running success reconciles archived
node records without pruning them, with existing bounded provider retries. Failed
apply reconciles the recovered old records. Session DNS confirmations clear on
commit. Uploaded validation/script diagnostics remain private and discarded; an
applied core emits its normal logs. Filesystem writes/fsyncs check cancellation
between bounded phases, but individual syscalls cannot be preempted. An interrupted
HTTP client must inspect status/revision before retrying because it may miss a
committed receipt.

Verification: `cargo check --workspace --locked --offline`, all 310 regular
workspace tests (including 20 regular backup tests), all 4 real-Mihomo backup
tests, warnings-denied Clippy, formatting and diff checks. New deterministic
coverage verifies same-PID reload, restart fallback and old-PID reaping, archived
node reconciliation, candidate startup failure with old data/core/node recovery,
and disconnect/HTTP-close/manager-shutdown cancellation during reload. Real-core
coverage switches mixed to HTTP listeners on the same port (verified restart),
then forces an occupied-port apply failure and verifies old listener/config/
catalog/settings/revision recovery. The actual 56-node archive test now performs
running API restoration and checks HTTPS 204 through stopped restore and service
restart, preserving original source/catalog bytes.

The runnable Linux MVP is refreshed at
`target/mihomo-server-linux-x86_64-live-restore`, with current Rust binaries,
unchanged Web assets, pinned independent Mihomo and updated deployment/provenance
docs. All 12 SHA-256 checksums pass; packaged binary and documentation match the
release/source files. Fresh-bundle smoke verifies a 1,110,883-byte/13-entry actual
56-node archive, live restore after a mode change with the same PID and restored
mode/node, stopped restore and service-restart persistence, and HTTPS 204 traffic
after each apply/restart. Original data-file hashes remain unchanged. No owned
service/core processes or disposable restore directories remain after cleanup.
No sandbox Git writes/commits occur; the host script owns the commit.

Next Delivery step 7 subtask: clean up private restore candidate directories left
by abrupt service termination using verifiable ownership, before extending local
archive retention/list/delete. Immutable source/revision garbage collection,
schedules, WebDAV, backup UI, full settings/resources, advanced pages, shared
components and additional supported release/service targets remain pending;
Windows compatibility stays deferred. The full project is not complete.

## Latest increment: scoped restore candidate leases and startup cleanup

Delivery step 7 completes abrupt-termination cleanup for new disposable restore
candidates. Rehearsal and publication now allocate private candidates under the
locked service data directory's reserved `restore-candidates` namespace instead
of global temporary storage. Each random `ms-restore-<24 lowercase hex digits>`
directory has a private, single-link regular `.lease` held under an exclusive
file lock for its entire lifetime. Startup cleanup runs after data locking and
before runtime/profile/settings initialization; durable restore-journal recovery
remains a separate step. Backup archives exclude this scratch namespace.

Cleanup requires an owned private no-follow root, strict candidate name/type/
ownership/mode and an available safe lease. Empty recognized directories without
a lease cover interruption between mkdir and lease creation. Live leases,
unrecognized entries, public/foreign directories, missing nonempty leases,
symlink/shared/nonregular leases and top-level candidate links are retained.
No global `/tmp` scan, PID/age heuristic or other service data directory is used.
Old unmarked temporary candidates cannot prove ownership and remain for manual
inspection. This is an explicit compatibility boundary, not broad file garbage
collection.

Enumeration and deletion are anchored to directory descriptors with no-follow
openat/unlinkat. Nested links/FIFOs are unlinked without traversing their targets;
owned nested directories must remain on the same filesystem, with identity checks
before removal. The lease is removed last, making partial deletion retryable.
Eligible private orphan permissions can be restored through descriptors; leased
active candidates are untouched. Each pass bounds directory enumeration and total
visits to 4,096 entries, depth 16 and a cooperative 15-second budget. Individual
syscalls are not preemptible. Unsafe root state or I/O/budget failure fails startup
closed for repair/retry while preserving remaining scratch and committed data.
No dependency changes or new upstream code copies are introduced.

Verification: `cargo check --workspace --locked --offline`, all 316 regular
workspace tests (21 regular backup API tests), all 4 real-Mihomo backup tests,
warnings-denied Clippy, formatting and diff checks pass. Five new unit tests cover
scope/live lease/empty creation gap, unsafe root and unverifiable entry retention,
anchored symlink/FIFO deletion, private orphan permission recovery and retry after
depth-budget failure. A new Linux integration test kills the service with SIGKILL
during a probe, verifies parent-death probe termination and leftover scratch,
then restarts the manager and verifies candidate cleanup with unchanged committed
runtime/settings/catalog and usable backup admission. The existing rehearsal and
restore cancellation/commit/rollback tests continue to pass at the new location.
An initial parallel library run hit an existing core-upgrade fixture's Text file
busy error; the complete final workspace run is serial and passes.

The runnable Linux MVP is refreshed at
`target/mihomo-server-linux-x86_64-candidate-cleanup`. All 12 SHA-256 checksums pass;
packaged Rust binary and deployment/provenance docs match release/source files.
Fresh-bundle smoke exports the actual 56-node archive (1,110,883 bytes/13 entries),
kills the release service during private restore preparation, and verifies startup
reclaims the candidate without changing committed configuration/catalog/settings.
It then verifies same-PID running restore, stopped restore and service-restart
persistence with selected-node HTTPS 204 traffic. Original data-file hashes remain
unchanged. Final cleanup leaves no owned service/core processes or candidates.
No sandbox Git writes/commits occur; the host script owns the commit.

Next Delivery step 7 subtask: implement local retained-backup storage and bounded
list/delete management, reusing verified snapshots and single operation admission.
Scheduled backup policy, WebDAV and backup UI follow. Immutable revision/source
garbage collection, full settings/resources, advanced pages, shared components
and additional release/service targets remain pending; Windows compatibility
stays deferred. The Linux MVP remains runnable and the full project is not complete.

## Latest increment: private retained-backup storage and management

Delivery step 7 now implements explicit local backup create/list/download/delete
on Linux. POST/GET `/api/backups` and GET/DELETE `/api/backups/{id}` accept empty
bodies under existing bearer/Host/Origin/no-query protections. IDs are generated
24-character lowercase hex values, never paths. Create reuses the serialized
current snapshot exporter, preserving running PID/configuration and stopped
semantics. The shared single backup slot spans storage workers and download bodies.

The reserved data-directory `backups` namespace is private (0700), with private
single-link regular files (0600). Strict generated filenames encode ID/time/SHA,
so listing reconstructs bounded metadata without a mutable sidecar index. Duplicate
IDs and unsafe recognized files fail closed; unknown entries remain untouched.
Lists expose only ID/time/size/digest and fixed count/byte limits, sorted newest
first. No host paths, profile contents/names/URLs or credentials are returned in
metadata. Downloads recheck identity/size/digest plus complete existing ZIP
inspection before streaming, and can use the normal explicit restore upload API.
Nested archives, partials and scratch files are excluded from snapshots.

Storage permits at most 32 committed archives and 256 MiB, with existing per-ZIP
65-MiB bounds and a 128-entry directory limit after bounded descriptor enumeration.
Capacity returns 507; no automatic deletion is introduced. Safe ID deletion is
idempotent (deleted=false if absent). Download absence returns 404. Invalid IDs/
bodies return 400, busy admission 409, storage failure generic 422 and interrupted
precommit work 503. Linux storage is implemented; other targets remain pending.

Create uses an exclusive private partial, fsync and atomic no-replace rename.
Rename is logical commit; create receipt reports committed=true, archive metadata
and durability_pending. Delete commits at unlink and reports deleted plus
durability_pending. Subsequent directory fsync errors never undo committed state.
Joined blocking workers receive shutdown/HTTP-close/disconnect cancellation and
a cooperative 15-second budget; syscalls remain non-preemptible. A client may miss
a commit receipt and must inspect the list before retrying. Startup after data
locking removes safe strictly named single-link private partials while preserving
committed files and unsafe/unknown entries. Unsafe roots or startup I/O/budget
failures fail closed for repair. No new dependencies or upstream source copies.

Verification: `cargo check --workspace --locked --offline`, all 321 regular
workspace Rust tests (including 23 regular backup API tests), all 4 real-Mihomo
backup tests, warnings-denied Clippy, formatting and diff checks pass. Three new
storage unit tests cover total byte capacity and entry bounds without pruning,
partial recovery preserving committed/unknown/unsafe files, and unsafe roots,
duplicate IDs and shutdown rejection. Two new API tests cover authenticated
create/list/download/restore, body-owned admission, private modes, unchanged
live snapshots, persistence and partial cleanup on restart, idempotent deletion,
ID/body/auth validation, corrupt ZIP rejection, safe link refusal, 32-archive
capacity and released admission. The actual 56-node restore test now retains and
downloads the real snapshot through the new API before running/stopped restoration
and selected-node HTTPS 204 through service restart. Original source/catalog
bytes remain unchanged.

The runnable Linux MVP is refreshed at
`target/mihomo-server-linux-x86_64-retained-backups`. All 12 SHA-256 checksums pass;
packaged Rust binary and deployment/provenance docs match release/source files.
Fresh-bundle smoke verifies private retained creation/list/download of the actual
56-node ZIP (1,110,883 bytes/13 entries), same-PID live restore, stopped restore,
service-restart persistence, exact retained re-download, idempotent deletion and
HTTPS 204 traffic. A SIGKILL during private restore preparation and a simulated
abandoned storage partial are recovered on startup while preserving retained
archives and committed configuration/catalog/settings. Original data-file hashes
remain unchanged. Final cleanup leaves no owned service/core processes or private
test storage/candidates. No sandbox Git writes/commits occur; the host script owns
the commit.

Next Delivery step 7 subtask: add persisted automatic backup policy and scheduling
with bounded pruning, explicit enablement and safe shutdown/restart behavior.
WebDAV, backup UI, immutable revision/source garbage collection, full settings/
resources, advanced pages, shared components and additional release/service
targets remain pending; Windows compatibility stays deferred. The Linux MVP
remains runnable and the full project is not complete.

## Latest increment: core-focused delivery priorities

The user stopped further backup expansion and set the active order to P1
configuration/resources, P2 rules/providers/delay tests, P3 i18n/signals and P4
Linux packaging plus actual systemd installation. The complete architecture tree
now distinguishes active pending work from deferred non-core work. Historical
backup next-task notes are superseded by the Active priorities and Delivery order
sections; delivered manual backup functionality remains intact.

The unshipped automatic-backup policy models, operation variant and unused policy
module from the interrupted increment were withdrawn. No scheduler, policy API,
pruning or runtime change was delivered in that increment. The working baseline
remains the verified Linux retained-backups MVP; this priority update does not
claim new features or actual systemd deployment.

Validation: `cargo check --workspace --locked --offline` passed using the existing
Cargo cache. The service settings integration suite passed its two regular tests;
eight opt-in real-Mihomo cases were not run for this documentation-only priority
update. Runtime sources match the previously verified baseline. No sandbox Git
writes/commits occur; the host script owns the commit.

Next task: P1 Geo/provider resource inventory, explicit paths/readback and
validation, followed by full authoritative settings and resource lifecycle.
The active priorities and the complete original design are not yet completed.

## Latest increment: committed Geo/provider resource inventory (P1)

The prior priority update is complete. This increment implements the first P1
resource-management slice: the authenticated `resources` command and a settings
page panel report the six upstream Geo filenames and explicit file/cache paths
from the committed `proxy-providers` / `rule-providers` configuration. The actor
captures the runtime revision and config together; reads also work while stopped
or before a runtime is committed. Bundle and writable Mihomo data roots are
reported separately. Provider lists use deterministic section/name ordering.

Metadata inspection identifies available, missing, empty, non-file, unsafe and
unreadable entries; inline resources, implicit core-managed cache paths and invalid
declarations are distinguished. Paths normalize relative to Mihomo's data root;
absolute paths inside that root are accepted. Parent traversal/external paths are
not inspected. Linux descriptor-relative `O_PATH`/`O_NOFOLLOW` walks reject links
without opening FIFO/device contents or following a raced parent directory outside
the root. Shared normalized paths, including collisions with Geo filenames, are
flagged without rewriting configuration. Reports omit provider URLs, headers,
inline payloads and file contents. Work is bounded to 512 providers, 512-byte names,
4096-byte paths and 64 path components.

The Web panel clears stale state on connection/revision changes, cancels abandoned
requests, handles authentication expiry and offers manual refresh/retry. It states
that file metadata is not content-format validation and that missing Geo files are
not universally required. No downloads, Geo replacement, provider refresh/reload,
candidate rejection, cache-conflict repair or new authoritative settings fields
are delivered here. Existing configuration acceptance remains unchanged. The
current development MVP runs with the built service and Web assets; the earlier
retained-backups release bundle does not contain this increment.

Validation:

- `cargo check --workspace --locked --offline` and formatting/diff checks passed.
- `cargo test --workspace --locked --offline -- --test-threads=1`: 325 passed,
  zero failed, 76 opt-in cases ignored. The first parallel attempt hit an existing
  Alpha activation test's transient `Text file busy` executable probe; the complete
  serial rerun passed without changing unrelated upgrade code.
- The opt-in `resource_inventory_live` integration test passed with real Mihomo
  and private temporary copies of actual subscription nodes and Geo data. The
  original source SHA-256 remained unchanged. Local proxy/rule providers loaded,
  Geo metadata matched, and HTTPS proxy traffic returned 204 before and after a
  core restart.
- `npm run build` passed, and the new Playwright resource inventory workflow passed
  against the built Web/service: missing → available → empty metadata, refresh,
  read failure and retry. The complete existing browser suite was not rerun.

No sandbox Git commit occurs; the host script owns the commit.

Next task: P1 provider candidate path validation and upstream-compatible shared
cache conflict handling, then Geo lifecycle and remaining settings management.
P2 rules/provider operations/delay views, P3 i18n/signals and P4 installed systemd
deployment remain pending; all previously deferred features remain deferred.

## Latest increment: provider candidate path authority and cache allocation (P1)

The previous authenticated inventory slice is complete. This increment adds
`headless-core::config::resource_paths`, adapting upstream's provider cache owner
grouping and `cvr-<sha256>[-n].<extension>` allocation. Explicit destinations normalize
against Mihomo's data root. Every URL in a multiple-source HTTP conflict receives
its own deterministic path; ordering cannot choose the winner. Same-URL HTTP caches
and shared local files remain supported. URL identity follows upstream exactly;
headers are not part of that identity. HTTP/local collisions are rejected, and
allocation avoids all declared destinations. Original paths/URLs remain in raw
subscriptions; only generated candidate paths change. No cache is deleted or moved.
This resolves simultaneous declarations; durable cache ownership across different
runtime revisions and implicit Mihomo cache identities remain resource-lifecycle
work, rather than being claimed as completed here.

The service applies this after runtime authority/finalization and before staging a
candidate or starting the Mihomo validator. Raw-profile edit validation uses a
normalized disposable revision, preserving the submitted source. Common probes,
core starts and hot reloads recheck paths without rewriting immutable committed
revisions. Invalid paths reject bootstrap/select/import application, manual edits,
overlays, enhancements and settings regeneration through the same actor stage.
Stored but inactive imports remain unapplied until selected. Legacy commits with
multiple HTTP sources sharing a path must be reapplied; startup does not silently
rewrite their revision.

The guard rejects traversal/external paths, links (including dangling/parent links),
nonregular files and hard-linked HTTP cache files. Reserved service roots/files,
Geo filenames, hidden paths and profile journal names are protected, as are the
current configuration/core executable and custom core directories below the data
root. Paths reject control characters, backslashes and colons, with the existing
4096-byte/64-component/512-provider bounds. Missing files remain Mihomo's validation
responsibility; inline resources and HTTP implicit cache paths are retained. Linux
metadata walks are shared with the inventory using pinned `O_PATH` descriptors.
These are preflight checks, not a filesystem sandbox around Mihomo: a trusted local
administrator can change files after a check, and Mihomo may create/download caches
during a valid probe even if a later configuration transaction fails. Runtime,
settings/catalog commits and source YAML retain their existing rollback semantics.

Validation:

- `cargo check --workspace --locked --offline`, `cargo build -p mihomo-server`,
  formatting and diff checks passed.
- `cargo test --workspace --locked --offline -- --test-threads=1`: 331 passed,
  zero failed, 76 opt-in tests ignored. Five new configuration tests cover allocation
  stability/order/collision suffixes, same-source/local sharing, bounded declarations,
  traversal/link/FIFO/hard-link/protected-path checks and later filesystem changes.
  The actor test verifies source preservation, settings regeneration, failed
  enhancement/raw/candidate edits, probe rejection and safe failed startup.
- The opt-in `resource_inventory_live` test passed with real Mihomo and private
  copies of actual nodes/Geo data. Two loopback HTTP subscriptions declared the same
  path; separate downloaded caches and both live provider node counts were verified.
  An unsafe overlay left PID/revision/source YAML intact. Proxy HTTPS returned 204
  before and after core restart, and the original node source SHA-256 was unchanged.
- The Playwright resource inventory refresh/retry workflow passed against the newly
  built service and existing Web assets. No Web source changed; the complete browser
  suite and ignored unrelated core integration tests were not rerun.

No sandbox Git commit occurs; the host script owns the commit.

Next task: P1 controlled Geo installation/content validation/initialization, then
Geo updates, full settings and remaining resource management. Provider refresh/
reload commands, rules/delay Web views (P2), i18n/signals (P3), actual Linux systemd
installation (P4) and previously deferred work are not completed by this increment.
The service remains a runnable development MVP; older release bundles do not yet
contain these P1 changes.

## Latest increment: integrity-pinned Geo seeds and first-use initialization (P1)

The prior provider-path increment is complete. This increment adds an optional
schema-1 `geo` map to the resource manifest and `--geo-dir`/`--geo-manifest` inputs
to the existing packager. Entries are limited to the six upstream Geo names and
contain exact byte size plus SHA-256. Old bundles without a `geo` map remain valid.
The manifest is bounded to 64 KiB, each seed to 128 MiB and the set to 256 MiB.
The packager checks original and copied seed pins and includes the files in the
existing bundle checksum inventory. It performs no resource download.

Under the data-directory lock, startup initializes the managed core and then Geo
files before spawning the actor/core. Only absent declared files are seeded.
Existing nonempty regular Geo files remain authoritative, even when bundle bytes
change or the seed source disappears; existing links, empty files or oversized
files fail initialization rather than being overwritten. Each needed source is
opened relative to its real directory without following links, streamed into a
private 0700 `.geo-seed` directory and verified against size/digest. All needed
files pass integrity checks before any live publication. No-replace hard links
publish independent 0600 copies, with data-directory fsync. A hash/size failure
before publication leaves live Geo names untouched. Publication is atomic per
file; an I/O error or abrupt exit partway through a set may retain already verified
files and the next startup seeds only the missing ones.

Interrupted staging recovery is confined to the fixed six-name namespace. Known
regular partial files are removed; unknown entries or links are preserved and
reported as unsafe. A staged hard link from an interrupted publication is removed
without changing its already published live file. No mtime-based overwrite,
resource deletion, HTTP download, authenticated update/upload API or settings
fields are delivered here. The existing resource panel observes the installed
files through its metadata refresh.

SHA-256 validates pinned content integrity; it is not a generic binary-format
validator. All six names can be seeded, but this increment's actual format/runtime
check covers `geoip.metadb` with real Mihomo: an IP-only request exercises its MMDB
loader through reject-only rules, followed by real proxy HTTPS traffic. Dedicated
MMDB/protobuf/ASN validation and controlled update/rollback remain pending.

Validation:

- `cargo check --workspace --locked --offline`, service build, formatting and diff
  checks passed. The regular workspace suite passed 336 tests, with 76 unrelated
  opt-in tests ignored. Geo tests cover pin/schema/size limits, failure before
  publication, source/destination links, private output, existing-data preservation
  and interrupted-staging cleanup.
- Nine Python packager boundary tests passed, including pin mismatch, paired Geo
  arguments, unsafe names/links and bounds.
- The real-Mihomo actual-node integration passed using private copies of node and
  Geo data. Startup seeded `geoip.metadb`; a reject-only IP request exercised the
  MMDB loader without an outbound connection. Local/HTTP providers remained usable,
  HTTPS proxy requests returned 204 before and after restart, and a changed bundle
  seed did not replace the installed Geo file. Original node and Geo SHA-256 values
  were unchanged. Other Geo formats were not exercised.
- A runnable local debug bundle was prepared at
  `target/mihomo-server-linux-x86_64-geo-seeds-1790519261902358745`; all 13 checksums
  passed. Its launcher reached Running, authenticated resource readback matched the
  Geo pin/private permissions, and SIGTERM exited cleanly with its core child reaped.
  This is a local verified artifact, not a completed production/systemd deployment.

No Web source changed; existing built assets are included, without a new browser
suite run. No sandbox Git commit occurs; the host script owns the commit.

Next task: P1 format-aware Geo validation and controlled replacement/update, then
full settings and cross-revision provider cache ownership. P2 operations/views,
P3 i18n/signals, P4 actual systemd installation and all deferred expansions remain
unfinished. Optional local Geo packaging supports this P1 slice and does not
complete the full P4 deployment/release scope.


## P1 increment: explicit MMDB verification and compatibility diagnostics

The previous Geo-seed increment is complete and retained. This increment adds
`service/src/geo_validation.rs`, the authenticated `validate_geo` command and
resource-page validation buttons for `Country.mmdb`, `ASN.mmdb` and
`geoip.metadb`. These are service adaptations, not a copied upstream command.

The lifecycle actor serializes validation with service mutations; filesystem and
parser work runs on a blocking worker. A fixed filename whitelist and descriptor
opens (`O_NOFOLLOW`, `O_NONBLOCK`, real data directory) reject arbitrary paths,
links and special files before reading. Inputs are nonempty regular files bounded
to 128 MiB and two million search nodes. File size/change timestamps are checked
around the bounded snapshot read. The report contains only filename, format,
bytes, SHA-256, IP version, node count, build epoch, `verified` and a fixed optional
warning; parser diagnostics/records never enter responses. Validation neither
changes runtime revisions/PID nor downloads or publishes files.

Pinned `maxminddb` 0.32.0 verifies metadata, search-tree structure, separator and
referenced data records with its bounded verifier. Invalid input returns 422.
Structural verification does not establish country/ASN record-schema compatibility,
rule coverage, freshness, or that the running core has loaded this exact snapshot.
External writers and Mihomo auto-updates can change the file after the read;
the SHA-256 identifies the verified snapshot, not a durable resource receipt.

**Real-data compatibility finding:** the current `data/geoip.metadb` has an empty
metadata description. Mihomo can use it, but the strict MaxMind verifier stops
before tree/data verification on that metadata condition. This exact condition
returns `verified: false` and `empty_description_structure_unverified`, displayed
as metadata-readable with full structure unverified. This is never a successful
structural check and never inferred to mean Mihomo cannot read the database. Other
verification errors remain failures. No Geo bytes or metadata are patched to
silence this difference. DAT validation and controlled update/recovery are pending.

The Web actions distinguish success, invalid-file errors and the compatibility
warning. Refresh, runtime revision/lifecycle changes, disconnect, logout and
unmount invalidate/abort pending browser requests and clear old results. The
metadata-only inventory remains inexpensive; opening the page does not trigger
full-file checks automatically.

Validation:

- `cargo check --workspace --locked --offline` and the service build passed;
  the regular workspace suite passed 339 tests, with 76 opt-ins ignored.
- Three MMDB boundary tests cover valid snapshots, corrupt tree/record/separator,
  missing descriptions, links/FIFO, empty/oversized files and filename policy.
  Authenticated command tests cover authorization, rejected overrides, invalid
  databases and unchanged runtime state.
- The actual-node opt-in test passed using private copies of the real Geo and
  subscription data. It received the expected metadata-only warning and matching
  SHA-256, retained PID/revision, exercised the Mihomo Geo loader, and returned
  HTTPS 204 through actual nodes before/after restart. Source data was unchanged.
- Web TypeScript/Vite build passed. The targeted Playwright resource workflow
  passed against the real service: invalid-file errors, success rendering,
  metadata-only warning rendering, refresh/retry and stale-result clearing.
  Success/warning rendering uses explicit API fixtures; actual Geo readback is
  exercised separately by the real-Mihomo Rust integration.

The complete architecture tree above now distinguishes delivered MMDB diagnostics
from pending DAT validation/update/settings. P2 operations, P3 i18n/signals and P4
actual systemd installation remain pending; backup expansion remains deferred.
No Git writes occur in the sandbox; the host owns the Conventional Commit.

Next task: controlled Geo replacement/update and recovery within P1, followed by
remaining full settings and provider cache ownership. Preserve the usable MVP and
complete these priorities before expanding other capabilities.


## P1 increment: stopped-core installation of pinned MMDB bundle resources

The previous read-only MMDB increment is complete. `geo_update.rs` now adds
explicit offline update inspection and installation, using the existing bundle
manifest and seed source. Authenticated `geo_seed` returns the current-file digest
(or null for a missing file) and pinned candidate size/digest. `install_geo_seed`
requires those expected digests; callers cannot supply arbitrary paths or URLs.
Only Country/ASN/MetaDB MMDB filenames declared in the manifest are supported.

The actor accepts installation only with phase Stopped and no managed process;
it serializes publication with lifecycle/configuration operations and joins the
blocking worker. Running, Failed, Recovering and other unsettled phases must be
explicitly stopped first. A failed or stale request does not change the committed
runtime revision, subscription/settings state or core generation. Bundle resources
and manifest pins are loaded at startup; a newly deployed manifest requires a
service restart before inspecting its new pins.

The installer uses the existing private `.geo-seed` namespace and bounded pin
copy. It checks candidate size/SHA-256 and MMDB parser outcome before publication,
then rechecks the current file's SHA-256. Nonempty or empty invalid current files
can be repaired while the manager is already available. Symlinks/special files
are rejected. A metadata-only candidate requires explicit `accept_metadata_only`
and the receipt keeps `verified: false`; this does not add structural verification
to the empty-description compatibility case. Identical valid candidate/current
content is idempotent and does not replace the inode.

Publication is one complete file: descriptor-based rename replaces an inspected
existing file; no-replace hard linking installs a missing file. The staged file
is synchronized before commit. Rename/link is the commit boundary, followed by
data-directory synchronization and staging cleanup. Ordinary precommit failures
leave the old destination intact. A receipt reports `durable` and `cleanup_pending`
so a postcommit sync/cleanup error is not misrepresented as an aborted update.

Startup reuses known-entry staging cleanup: an interrupted unpublished candidate
is removed and authoritative data is retained; a committed candidate remains the
new data. Unknown/unsafe staging entries are retained and reported. No retained
backup, multi-file transaction, automatic rollback after a later core start, online
download or running-core restart transaction is claimed here. External writers
must respect the service data-directory lock; digest guards protect serialized
service updates, not an OS-level compare-and-swap against arbitrary writers.

The Resources panel exposes candidate/current hashes, stopping-state restrictions,
explicit metadata-only acceptance and result readback. Failed installation clears
the candidate state and requires inspection again. Successful installation refreshes
inventory and keeps its receipt visible. Lifecycle/revision/connection changes and
logout clear pending browser state. Aborting a browser request after installation
begins does not cancel/undo a filesystem commit; inspect again after an interrupted
response. This is an explicit adaptation of upstream bundled Geo copying, avoiding
its automatic modification-time overwrites.

Validation:

- `cargo check --workspace --locked --offline`, service build and the regular
  workspace suite passed: 344 tests, 0 failures, 76 opt-ins ignored.
- Four installer boundary tests cover missing installs, guarded repair, idempotence,
  private permissions, hardlink alias preservation, stale file/seed digests,
  corrupt pinned formats, links, explicit compatibility acceptance and interrupted
  staging recovery. Existing ten Geo/core initialization tests still pass.
- Authenticated HTTP tests exercise a real strict-valid MMDB installation, unchanged
  runtime state, stale-request rejection, missing bundle resources, authentication
  and forbidden source overrides.
- The actual-node integration passed with private node/Geo copies: running-core
  rejection, tampered candidate preservation, stale digest rejection, explicit
  metadata-only restoration of a damaged isolated MetaDB, clean staging, unchanged
  runtime revision and usable proxy HTTPS 204 before and after stopping/installing/
  starting. Original subscription and Geo hashes were unchanged.
- TypeScript/Vite build and two targeted Playwright workflows passed. Browser update
  controls use API/WebSocket fixtures to verify running-state disabling, fresh
  inspection, exact digest fields, failure cleanup and retained compatibility
  warnings. Actual file publication is verified by HTTP/Rust/live-core tests.

The complete tree above distinguishes delivered offline MMDB installation from
remaining Geo formats/settings/online updates. P2 rules/providers/delay views,
P3 i18n/signals and P4 actual systemd installation remain pending; unrelated
backup expansion stays deferred. The host script owns Git commits.

Next task: authoritative Geo settings and readback within P1. This enables
configuration-controlled Geo mode/URL/automatic-update behavior before adding
online update transactions. DAT validation and provider cache ownership remain
named P1 work; the usable MVP is preserved.


## P1 increment: authoritative Geo settings and actual core readback

The stopped-core bundle installation increment is complete and retained. This
increment adds Geo settings to the existing schema-one runtime store rather than
creating another settings file or publication path:

- `geodata-mode`: optional bool (false selects MMDB, true selects DAT).
- `geodata-loader`: optional standard/memconservative enum.
- `geo-auto-update`: optional bool; false remains an explicit owned value.
- `geo-update-interval`: optional integer, 1–8760 hours (service policy).
- `geox-url`: optional geoip/geosite/mmdb/asn URL leaves. Each supplied URL must
  be a nonempty HTTP(S) address with a host, no user credentials/fragment/whitespace,
  and at most 8192 bytes. Loopback/private HTTP URLs are supported; query strings
  are allowed. Validation errors do not echo URL values.

Absent/null leaves inherit independently. An empty geox-url map owns no leaves;
provided URLs shallowly override only their own entries, preserving other source
and enhancement keys. Explicit fields enter the initial generation and are enforced
again after scripts/manual overlays. Owned-leaf warnings identify discarded
`geox-url.<leaf>` changes. Saved source YAML remains unchanged. Removing authority
regenerates an active subscription; for standalone runtime edits, existing values
remain until explicitly edited, matching the established settings semantics.

`set_settings` continues using the actor-owned generation/probe/application/settings
journal/runtime commit transaction. Failed core probes do not change settings,
committed revision or the running PID. Stopped updates stay stopped, and old
schema-one files with no Geo fields remain valid. The aggregate serialized settings
size is now checked before starting a journal, preventing a valid individual Geo
URL combined with large network settings from leaving an oversized unrecoverable
transaction. The existing 64-KiB publication limit is retained.

Authenticated `geo_settings` runs in the same actor. It returns eight named leaves
with saved setting, committed configured value, actual core value and mismatch.
A configured omission remains distinct from a known core default. Actual readback
uses the retained client GET /configs with a three-second bound; stopped cores
return unknown actual values and a running read failure reports a fixed error
without inventing values. The client now accepts native `geoip`/`geosite` response
keys and retained `geo-ip`/`geo-site` aliases while preserving camelCase output.

The Web settings editor supports every delivered Geo field and preserves existing
network/scalar settings during full replacements. Per-URL inheritance has its own
container toggle. Saving includes confirmed settings readback. The Geo comparison
panel refreshes on settings/lifecycle/revision changes, distinguishes inherited
fields and actual mismatches, and clears stale results on failure/disconnect.
Unknown settings remain protected from being silently dropped by the editor.

**Scope boundary:** this configures Mihomo's native automatic-update behavior;
it does not add a service-managed download/stage/update/rollback transaction.
Enabling it lets Mihomo mutate resources itself. Switching modes does not prove
DAT/MMDB resources or record schemas are valid, and settings GET /configs does
not prove that a rule has loaded a database. Geosite matcher selection, general
DAT validation, controlled live/online Geo updates and other full settings remain
pending. The previously delivered offline pinned installer is unchanged.

Validation:

- `cargo check --workspace --locked --offline`, the service build and formatting
  checks passed. Workspace tests passed: 351 regular tests, zero failures and 77
  opt-in tests skipped by default; the two relevant real-core tests below were
  explicitly enabled and passed separately.
- Three Geo authority/schema tests plus aggregate-size protection cover per-leaf
  preservation, explicit false, empty maps, enums/URL bounds and secret-free errors,
  schema-one restart and interrupted settings recovery. Client alias and service
  comparison tests cover real field keys, defaults, mismatches and unavailable cores.
- Authenticated HTTP integration covers preconfiguration saved values, authority
  across subscription/merge generation, unowned URL preservation, invalid-input
  rejection with unchanged state, inheritance restoration and unchanged raw YAML.
- The explicit real-Mihomo settings integration passed: initial/final script
  authority, failed -t probe rollback with unchanged PID/revision, stopped saves,
  service restart and matching actual core values.
- The actual-node resource integration passed using private copies of data: all
  eight saved/configured/actual leaves matched, including native geoip/geosite
  URLs; MetaDB validation/install and HTTPS proxy 204 remained usable after restart,
  with original node/Geo source hashes unchanged. Automatic update stayed disabled
  and no test Geo URL was fetched.
- TypeScript/Vite build and the full Playwright suite passed (25 workflows,
  four optional checks skipped). The Geo editor saves through the real service,
  rejects bad drafts locally and restores
  inheritance; explicit response fixtures exercise read failures and mismatch
  rendering. Existing settings editor and DNS/TUN inheritance workflows also
  passed in their complete setup sequence.

The complete tree is updated above. No dependencies, Git commits, backup expansion
or P2/P3/P4 work are added. Next task: P1 durable provider cache ownership across
revisions, preserving working node traffic and source data while preventing stale
cache reuse across changed provider sources. Remaining P1 Geo formats/controlled
updates/settings are still pending before advancing to P2.

## P1 increment: HTTP provider cache ownership across revisions

The previous Geo authority increment is complete. Candidate preparation now adds
source-addressed HTTP cache paths after the existing upstream-compatible conflict
allocator. Both proxy and rule HTTP providers, including declarations without a
path, receive `provider-cache/v1/<sha256>.cache`. The reserved namespace is only
available to HTTP providers; file providers cannot claim it. Local file paths and
inline declarations retain their behavior. Explicit original paths are still
checked for unsafe destinations before being rewritten; no source YAML changes.

The versioned SHA-256 identity contains the provider section, URL, header, proxy,
format and behavior in deterministic JSON order. URL/header/transport/parser
changes select a different file, even when only one provider exists. Provider
names, intervals, health checks, filters, overrides and original path spelling do
not change the identity of the downloaded raw file. Identical identities share a
cache; differing proxy/rule parser identities do not. Absent fields and explicitly
specified defaults may conservatively select separate caches. Private URL/header
values are hashed, not included in filenames or inventory responses.

Ownership is persistent in the versioned source-addressed name and committed
configuration; it requires no mutable owner ledger, no additional journal and no
cache-file rename/delete. A probe may download a candidate cache before its runtime
transaction fails, but a different source cannot overwrite a previously owned
file. Changing content at the same source remains Mihomo's refresh responsibility;
this is source ownership, not content integrity or a provider refresh command.
Old caches and unclaimed original files are retained without being imported.
Automatic pruning and content verification are not added.

The actor uses this policy for all staged candidates and raw-edit validation copies.
Probes, start and reload recompute and check ownership without rewriting immutable
commits. Legacy HTTP revisions (including implicit paths and old cvr aliases) must
be explicitly reapplied by selecting the subscription or editing/applying the
configuration. Startup fails safely until this is done. Initial migration may need
a working source download; old cache presence does not authorize its reuse.
Existing confined metadata, protected paths, symlink/hard-link checks and provider
bounds also apply to the generated namespace. These are preflight checks under the
service data lock; trusted external writers must respect the lock and namespace.

Validation:

- `cargo check --workspace --locked --offline` and the service build passed.
  Workspace tests passed: 354 regular tests, zero failures and 78 opt-in tests
  ignored by default. Formatting and diff checks passed.
- Three new ownership tests cover cross-revision source changes, idempotency,
  renamed/retimed providers, canonical header ordering, credential/transport/parser
  separation, implicit HTTP paths, legacy rejection, protected namespaces, local
  claims and later symlink/hard-link changes. Existing allocation tests remain.
- Authenticated actor/inventory tests cover stable regeneration, unchanged raw
  YAML, rejected edits before probes, generated cache metadata, URL/header
  redaction, missing-file refresh and start-time path rechecks.
- Explicit real-core `provider_cache_live` passed: a single provider switched from
  Alpha to Beta using distinct files; a rejected candidate retained PID/revision
  and both caches. An implicit HTTP rule provider loaded one classical rule.
  With the download server closed, service restart and subscription reselection
  reused the correct node/rule caches; raw sources remained unchanged.
- Explicit actual-node `resource_inventory_live` passed using private data copies:
  HTTP provider caches loaded, resource/Geo operations remained usable and HTTPS
  proxy traffic returned 204 before and after restart. Original node and Geo source
  hashes were preserved.
- The Playwright resource inventory refresh/retry workflow passed against the newly
  built service. No Web source changed; unrelated browser workflows were not rerun.

The complete architecture tree is updated above. The development MVP remains
available; existing HTTP revisions need explicit reapplication as described.
Next task: P1 geosite matcher settings and readback, followed by remaining settings
and Geo format/lifecycle work. P2 operations/views, P3 i18n/signals, P4 actual Linux
systemd installation and previously deferred features remain unfinished. No Git
commit is performed inside the sandbox.

## P1 increment: geosite matcher authority and core readback

The provider cache ownership increment is complete. Optional `geosite-matcher`
joins schema-one runtime settings with typed canonical `succinct` and `mph` values.
Missing/null values inherit subscription/enhancement values; older settings files
continue to deserialize without a schema change. Service-owned matcher values enter
before scripts and are enforced again after scripts, merges and overlays. The
existing settings/runtime transaction and recovery protect failed applications.
Invalid variants and wrong types fail before a settings transaction. Original
subscription YAML is preserved.

The authenticated `geo_settings` comparison adds a ninth named field at the end,
preserving the order of the eight existing fields. Saved, committed and actual
matcher values use the retained Mihomo BaseConfig model and bounded actor-owned
GET /configs. Missing/empty matcher responses from older cores appear as unknown,
not an invented default or mismatch. Inherited core defaults are distinct from
configured mismatches. The Web editor provides inherit/succinct/mph choices,
confirmed saving and readback, with existing refresh/retry and stale-state clearing.
Both string enums remain distinct from Geo boolean fields.

Mihomo also accepts a legacy `hybrid` alias in source YAML; the service's explicit
settings accept only the two canonical names. Inherited source values are not
rewritten. This increment configures the matcher and verifies reported settings;
it does not validate geosite.dat, exercise GEOSITE rule matching or install/update
DAT files. Those resource-lifecycle responsibilities remain pending.

Validation:

- `cargo check --workspace --locked --offline`, the service build and formatting
  checks passed. Workspace tests passed: 356 regular tests, zero failures and 78
  opt-in tests skipped by default. The two relevant real-core tests below were
  explicitly enabled and passed separately.
- Matcher authority tests cover both canonical variants, absent/null inheritance,
  unmodified legacy source values, override diagnostics and invalid/wrong-type
  rejection. Existing schema-one transaction/recovery coverage now includes mph.
  Snapshot tests distinguish mismatch, inherited defaults and missing core fields.
- Authenticated HTTP tests verify preconfiguration saves, merge authority, invalid
  input rejection without state changes, committed matcher readback, inheritance
  restoration and unchanged original YAML.
- Real Mihomo settings integration passed: initial/final script authority, running
  succinct/mph switches with actual readback, failed probe rollback retaining
  PID/revision/actual values, stopped saves and service restart persistence.
- Actual-node integration passed using private data copies: all nine Geo values
  matched the core, HTTPS proxy traffic returned 204 before/after restart, and
  original node/Geo source hashes remained unchanged. This verifies configuration
  readback and usable proxy traffic, not DAT/GEOSITE matching.
- TypeScript/Vite production build and three resource/Geo Playwright workflows
  passed. The matcher editor saves a string enum and confirms its value on reload,
  restores inheritance, renders mismatch fixtures and clears stale rows on read
  failures. Unrelated browser workflows were not rerun.

The complete architecture tree is updated above; the Linux development MVP remains
runnable. Next task: P1 TCP concurrency and process matching settings, followed by
other remaining configuration and Geo lifecycle work. P2 rules/provider/delay
operations/views, P3 i18n/signals, P4 actual Linux systemd installation and deferred
features remain incomplete. No sandbox Git commit is performed.

## P1 increment: TCP concurrency and process matching settings

The geosite matcher increment is complete. Optional `tcp-concurrent` and
`find-process-mode` now join schema-one service runtime settings. The TCP value is
a strict boolean; explicit false remains authoritative. Process modes are typed
canonical `strict`, `always` and `off`; missing/null fields inherit. Both values enter
before scripts and win after scripts, merges and overlays. Existing settings/runtime
journals protect interrupted saves and failed candidate application without changing
subscription source YAML. Older settings files continue to load without a migration.

Authenticated `connection_settings` returns the same committed comparison envelope
as Geo readback with two whitelisted leaves. It reads the core from the serialized
actor with a three-second timeout, reports stopped or failed readback as unknown,
and keeps saved/committed values available. A narrow client GET /configs projection
uses optional fields to distinguish absence from false/off; native lower/title-case
process modes normalize to canonical lower-case names. Unsupported/missing fields
do not acquire BaseConfig's legacy defaults. Read errors use a fixed diagnostic.

The backend per-leaf comparison and Web refresh/readback panel are shared with the
Geo view, retaining its nine fields and order. The Web settings form adds TCP and
process mode choices with inheritance, lossless full replacement, confirmed saved
values and draft preservation on errors. Both comparison panels clear stale values
on failures, disconnects and lifecycle/revision/settings changes.

This verifies configured and core-reported values and usable proxy traffic; it does
not prove that a specific process was identified or that concurrency improves
performance. Process rules/results and richer connection dashboards are not added.

Validation:

- `cargo check --workspace --locked --offline`, the service build and formatting
  checks passed. Workspace tests passed: 359 regular tests, zero failures and 79
  opt-in tests skipped by default. The two relevant real-core tests below were
  explicitly enabled and passed separately.
- Pure settings tests cover explicit false, all canonical process modes,
  absent/null inheritance, invalid types/variants and unchanged unrelated fields.
  Existing interrupted-publication recovery now includes both connection fields.
  Comparison tests cover native mode casing, missing old-core fields, mismatches,
  inherited defaults, stopped state and read failure; existing Geo comparisons
  verify the shared helper preserves their behavior.
- Authenticated HTTP integration verifies preconfiguration saves, merge/overlay
  authority, invalid input rejection with unchanged saved state/revision,
  committed readback, inheritance restoration and unchanged raw YAML.
- Explicit real-Mihomo connection integration passed: initial/final script
  authority, enabled/disabled TCP and all process modes, core readback, failed probe
  rollback retaining PID/revision/settings/actual values, stopped saves and service
  restart persistence.
- Explicit actual-node integration passed with private data copies: TCP/process
  values matched the core, resources/Geo remained usable and HTTPS proxy traffic
  returned 204 before/after restart. Original node and Geo hashes were unchanged.
- TypeScript/Vite build and the full Playwright suite passed: 26 workflows with
  four optional upgrade/repair checks skipped. Coverage includes real connection
  settings save/reload/inheritance, failed-draft preservation, read failures,
  mismatch rendering, stale-row clearing and the existing Geo/network editors.

The complete architecture tree is updated above. The Linux development MVP remains
runnable. Next task: P1 TCP keep-alive interval/idle/disable settings and readback,
then other remaining configuration/Geo lifecycle work. P2 rules/provider/delay
operations/views, P3 i18n/signals, P4 actual Linux systemd installation and previously
deferred features remain incomplete. No sandbox Git commit is performed.

## P1 increment: TCP keep-alive settings and readback

The TCP concurrency/process matching increment was complete before this task.
Optional `keep-alive-interval`, `keep-alive-idle` and `disable-keep-alive` now join
schema-one runtime settings. Durations accept signed 32-bit integer seconds;
explicit zero/negative values and false are authoritative, while missing/null
fields inherit. This service integer bound avoids duration multiplication overflow;
it is not an upstream or OS socket limit. Initial/final enforcement prevents
scripts, merges and overlays from overriding saved settings without rewriting raw
subscription YAML. Existing settings/runtime transactions cover recovery and
failed application; older schema-one files still load without migration.

The authenticated, actor-serialized `connection_settings` response appends the
three new leaves to TCP concurrency/process mode. Narrow optional 64-bit core
values preserve zero, negative and missing fields. Saved/configured/actual values,
mismatch logic, stopped-state unknowns, timeouts and retry reuse the shared
comparison envelope. The Geo view keeps its existing nine fields and ordering.
The Web editor provides integer-second inputs, disable/inherit choices, validation,
saved summaries, confirmed replacement and draft preservation on failures.

Modern Mihomo forwards these fields to Go's KeepAliveConfig: zero uses a default,
negative preserves the corresponding socket option, and the disable flag disables
probes. Core versions/platforms may differ. This increment verifies configuration
and core readback plus usable traffic; it does not claim packet timing or per-socket
OS application. Upstream/source links and semantics are recorded in UPSTREAM.md.

Validation:

- `cargo check --workspace --locked --offline`, service build, formatting and diff
  checks passed. Workspace tests passed: 360 regular tests, zero failures and 79
  opt-in tests skipped by default. The two relevant real-core tests below were
  explicitly enabled and passed separately.
- Pure tests cover signed integer boundaries, zero/negative durations, explicit
  false, missing/null inheritance, invalid types/out-of-range values, initial/final
  enforcement, preserved unrelated fields and settings round trips. Existing
  interrupted-publication recovery now includes all three keep-alive fields.
- Comparison tests cover mixed-version partial/missing fields, preserved zero/false,
  mismatches and read failure. Authenticated HTTP integration covers saves before
  configuration, merge/overlay authority, rejected invalid updates with unchanged
  state/revision, inherited values and unchanged raw YAML.
- Explicit real-Mihomo connection integration passed: zero/negative and positive
  duration readback, disable false/true, initial/final script authority, hot updates,
  failed probe rollback retaining PID/revision/settings/config/actual values,
  stopped inheritance and persisted settings after service restart.
- Explicit actual-node resource integration passed with private copies: all five
  connection settings matched the core, Geo/provider resources remained usable,
  HTTPS proxy traffic returned 204 before/after core restart, and original node/Geo
  hashes remained unchanged.
- TypeScript/Vite build and the full Playwright suite passed: 26 workflows, four
  optional upgrade/repair checks skipped. Coverage includes numeric rejection,
  zero/negative/false save and reload, preserved failed drafts, inheritance clearing,
  readback failure/retry/mismatch handling and existing scalar/DNS/TUN/Geo editors.

The complete architecture tree is updated above. The Linux development MVP remains
runnable. Next task: P1 outbound interface-name and Linux routing-mark authority,
Web controls and readback, then remaining configuration/Geo lifecycle work.
P2 rules/provider/delay workflows, P3 i18n/signals and P4 actual systemd installation
remain incomplete; all previously deferred work stays deferred. The external host
handles Git commits; no sandbox Git commit is performed.

## P1 increment: outbound interface and Linux routing mark

The previous TCP keep-alive task was complete and the worktree clean at the start
of this increment. Optional interface-name String and Linux-only routing-mark u32
now join schema-one service settings. Strict deserialization prevents YAML
number/boolean-to-string coercion. Empty name explicitly clears a fixed
interface; mark 0 clears the default mark. Null/omitted leaves inherit. Names follow
a 15-byte UTF-8 Linux primary-name subset, without whitespace/control characters,
slash/colon or dot names. This validates syntax, not presence or reachability.
Original source YAML remains unchanged; owned values apply before scripts and win
after merges/scripts/overlays. Existing settings/runtime journals cover recovery.

The authenticated actor-serialized connection comparison appends interface-name
and routing-mark to its five existing leaves. Optional String/i64 actual values
preserve empty/zero and old-core absence. Unknown fields do not acquire default
values, and read failures/timeout/stopped states retain the shared behavior.
The Geo comparison retains its existing fields/order. Core readback verifies
reported defaults, not per-socket interface binding or privileged kernel marking.
Raw signed core marks are preserved; equivalent signed/unsigned 32-bit values are
compared by their bits, without truncating out-of-range values. Explicit empty
strings display as `""` rather than disappearing from the shared Web comparison.
Full-range nonzero mark readback is verified on Linux x86_64, not 32-bit cores.
Per-proxy/provider settings and system policy can affect individual sockets.

The Web outbound-settings module adds an ownership checkbox and exact interface
name input, distinguishing inherited and explicit empty names. Mark input supports
blank inheritance and explicit zero/full u32 values. Validation, saved snapshots,
confirmed save/reload, failed draft retention and existing settings are preserved.

Validation:

- `cargo check --workspace --locked --offline`, the service build, formatting and
  diff checks passed. Workspace tests passed: 362 regular tests, zero failures,
  79 opt-in tests skipped by default. The two relevant real-core cases below were
  explicitly enabled and passed separately.
- Pure settings tests cover strict YAML string and integer types, UTF-8 byte
  boundaries, invalid/control/whitespace names, exact empty names, null inheritance,
  zero and full-u32 marks, initial/final authority, preserved unrelated fields and
  schema-one interrupted-publication recovery. Comparison tests preserve raw signed
  actual marks, compare only valid 32-bit representations, reject out-of-range
  truncation and preserve missing old-core fields.
- Authenticated HTTP integration covers seven-field readback, saves before a
  configuration exists, source/merge/overlay authority, invalid JSON input rejection
  without changing saved state or revision, restored inheritance and raw YAML.
- Explicit real-Mihomo integration passed: script input/final authority, empty/lo
  names, zero/full-u32 core-reported marks, native signed readback, live updates,
  failed probe rollback preserving PID/revision/settings/config/actual values,
  stopped inheritance and persistence after service restart.
- Explicit actual-node resource integration passed using private copies and the
  host's existing `wlo1` interface. All seven connection settings matched before
  and after core restart, Geo/provider resources stayed usable, HTTPS proxy traffic
  returned 204 (including through the configured interface after restart), and
  original node/Geo hashes stayed unchanged. Mark 0 was used for traffic; this
  does not verify privileged nonzero SO_MARK or install host route policy.
- TypeScript/Vite build and the full Playwright suite passed: 26 workflows with
  four optional upgrade/repair cases skipped. Coverage includes name/mark rejection,
  checked empty-name persistence, full-u32 save/reload, inheritance clearing, failed
  drafts, shared empty-string display and readback failure/retry/mismatch handling.
  Existing scalar, network and Geo editors also passed. The targeted connection
  workflow passed again against the final outbound fieldset styling.

The complete architecture tree is synchronized above; the Linux MVP stays runnable.
Next task: P1 global download User-Agent/ETag authority, Web editor and readback,
followed by remaining configuration/Geo lifecycle work. P2 rules/provider/delay
workflows, P3 i18n/signals and P4 actual Linux systemd installation remain incomplete.
Deferred features remain deferred. The external host commits changes; no sandbox
Git commit is performed.

## Increment: global core download User-Agent and ETag settings

Completed the next P1 task specified by Delivery order. The existing schema-one
runtime settings now accept optional `global-ua` and `etag-support`. Absent/null
inherits; `""` explicitly clears User-Agent and false disables ETag. Owned values
are applied before enhancement and after scripts/merges/overlays through the
existing settings authority. Original subscriptions, provider identity/cache
allocation, subscription download options and persistence formats are preserved.

User-Agent must be an actual string with at most 1024 printable ASCII bytes,
including spaces; YAML numeric/boolean coercion, non-ASCII and control/header
injection characters are rejected. The bound is a service policy. ETag is a strict
boolean. Interrupted save recovery now exercises owned empty/false download values.
The narrow client projection adds optional leaves, and the existing actor comparison
shows nine connection/outbound/download fields without inventing missing core
values or changing signed routing-mark handling. Shared comparison and timeout/
retry behavior are reused.

`web/src/download-settings.tsx` adds **核心下载设置**, a User-Agent ownership checkbox
that preserves an explicitly empty input, a three-state ETag select, validation
and a saved snapshot. Saving/rereading, failed drafts and full inheritance reset
preserve other runtime fields. The existing connection panel reads both leaves.
The UI distinguishes core resource downloads from the service's subscription
`user_agent` option. Per-resource nonempty User-Agent headers may override the
core global value; readback alone is not proof of actual request behavior.

Verification:

- `cargo check --workspace --locked --offline`, the Web production build,
  formatting and whitespace checks succeed. Workspace tests report **363 passed,
  80 opt-in ignored**. The new pure settings case covers empty/custom/max-length
  strings, false/null values, invalid header/type bounds, authority and roundtrip;
  existing recovery, authenticated commands and presence/mismatch cases extend
  coverage without new dependencies.
- Explicitly enabled real-core settings workflow passes initial/script/final
  authority, mode changes, failed application rollback, restored inheritance,
  stopped saves and service restart, including both new download fields.
- A new local HTTP provider fixture passes against `/usr/bin/verge-mihomo`: actual
  global User-Agent, provider-specific override, enabled ETag cache warmup followed
  by If-None-Match/HTTP 304, disabled conditional requests, explicit empty agent
  and core restart. The retained client update call is used only by the harness;
  P2 provider-management commands/views remain pending.
- Explicitly enabled live-node resource workflow passes through the existing
  `wlo1` interface using private subscription/Geo copies. All nine settings match
  before/after core restart and the HTTPS proxy request returns 204. Original node
  and Geo SHA-256 fingerprints remain unchanged.
- Full Chromium regression reports **26 passed, 4 optional bundle upgrade/repair
  workflows skipped**. The extended connection workflow verifies printable ASCII
  and length validation, explicit empty/false save/readback, failed drafts,
  confirmed custom/true save, rereading and complete inheritance restoration.

The complete architecture tree is synchronized above, and the MVP remains runnable.
Next task: P1 hosts configuration authority and DNS host-use controls, Web editing
and real DNS behavior verification. Remaining settings, DAT validation and controlled
Geo updates stay in P1. P2 rules/provider/delay, P3 i18n/signals and P4 actual Linux
systemd installation remain incomplete; unrelated work remains deferred. The host
script commits this increment; no sandbox Git commit is performed.

## Increment: hosts configuration and DNS host-use controls

Completed the next P1 task in Delivery order. New
`crates/headless-core/src/config/settings/hosts.rs` models strict scalar IP/alias/lan
and IP-list values while retaining source spelling and scalar/list shape. Optional
`RuntimeSettings.hosts` owns the entire table; absent/null inherits and `{}` clears
configuration mappings. Hosts applies with the DNS stage after TUN derivation,
before enhancements, and again during final authority, including overlays. Override
diagnostics report `hosts` for whole-table changes, including removal/empty tables.
Schema-one save/recovery preserves empty hosts and false DNS host-use values.

The service accepts a bounded ASCII domain-pattern subset (wildcards and leading
`.`/`+.` suffix patterns, punycode for IDNs), at most 1024 entries and 1–64 IPs per
list, within the existing whole-settings 64 KiB bound. It rejects malformed/empty
patterns, YAML scalar coercion, non-IP lists, case-duplicate keys and potential
alias cycles, conservatively including wildcard matches even if shadowed. These
are service input policies; not all upstream permissive syntax is exposed.

`DnsSettings` adds `use-system-hosts`; both it and `use-hosts` preserve authoritative
true/false. This is an intentional extension of the upstream DNS page's true/
nonempty rule. Other DNS false/empty inheritance remains unchanged. Hosts and DNS
share the existing provider-policy digest, profile preference and session-scoped
confirmation. Hosts-only saves can request/confirm an override; denied or expired
permissions suppress both DNS and hosts at regeneration, retaining source values.
Persisted committed revisions keep the existing restart/reapplication semantics.
Neither host files nor system DNS/routes are modified.

`web/src/hosts-settings.tsx` adds **hosts 映射**, explicit ownership, a typed JSON
editor, input validation and a saved snapshot. The DNS fieldset includes both
host-use selects, and hosts-only settings enable the existing subscription
confirmation panel. Failed drafts, explicit empty tables, rereading and full
inheritance restoration work with whole-runtime replacement. Browser host-map
ordering normalizes to the backend BTreeMap for save/dirty comparison; a browser
regression exposed and fixed false mismatch reporting for reordered equivalent
maps. IP list ordering and scalar/list shapes remain significant. The existing
configuration page/API exposes generated values. No actual hosts/host-use fields
are fabricated from GET /configs; DNS behavior is verified directly.

Verification:

- Workspace compilation and Web production build succeed. Workspace tests report
  **365 passed, 81 opt-in ignored**. Two new pure cases cover types, bounds,
  patterns/aliases, scalar/list roundtrip, whole-map clearing, initial/final
  authority and false-switch semantics. Existing journal recovery and authenticated
  full-replacement command checks now cover hosts and both DNS switches.
- The explicitly enabled real-core UDP workflow passes against
  `/usr/bin/verge-mihomo`, with a local deterministic upstream. Exact/wildcard
  priority, alias resolution, A/AAAA and mixed IP lists, configured-host disable,
  system-host enable/disable using an existing entry, initial script inputs/final
  ownership, semantic failure rollback, stopped-overlay authority, empty tables,
  released inheritance, hosts-only provider confirmation and service restart are
  verified. `/etc/hosts` bytes and original subscription YAML remain unchanged.
- The existing real-core provider DNS confirmation and refresh/recovery workflows,
  and network authority/rollback workflow, are explicitly enabled to check
  compatibility with the extended gating and host booleans.
- The actual-node resource workflow passes using private subscription/Geo copies
  and `wlo1`; HTTPS proxy traffic returns 204 after core restart and original node/
  Geo hashes remain unchanged.
- Full Chromium regression reports **27 passed, 4 optional bundle upgrade/repair
  workflows skipped**, verifying the new hosts editor alongside existing settings,
  network/provider confirmation and profile workflows. It covers invalid
  input rejection, map shape/canonical comparison, false switches, explicit empty
  save/reread, failed drafts and released inheritance. The hosts workflow is also
  rerun against the final production bundle after clarifying the hosts-only
  confirmation guidance and adding a hosts-only saved-state check.

The complete architecture tree is synchronized above and the Linux MVP remains
runnable. Next task: P1 DAT resource validation and compatibility diagnostics with
management/Web checks. Remaining full settings, native TUN and controlled Geo
updates stay pending P1; P2 rules/provider/delay, P3 i18n/signals and P4 actual
systemd installation remain incomplete. Deferred features remain deferred, and
Git submission is left to the external host script.

## Increment: bounded DAT validation and compatibility diagnostics (P1)

Completed the next task in Delivery order. New `service/src/dat_validation.rs`
validates the GeoIPList/GeoSiteList protobuf schema through a bounded byte-slice
reader, without new dependencies or retaining record payloads. Known fields check
wire types, truncation/overflow, CIDR family/prefix, domain enums, nonempty UTF-8,
attribute bool/int64 oneof and proto3 defaults. Limits bound file size, groups,
records, total fields and text lengths; service policy rejects duplicate singular
fields and case-duplicate group identifiers. Unknown fields are skipped safely
and produce an explicitly unverified result. Fixed schema nesting bounds recursion.

The existing authenticated actor `validate_geo` command now accepts `geoip.dat`
and `geosite.dat` alongside the three MMDB names. It reuses confined no-follow
regular-file snapshots, the 128 MiB bound, read-time metadata guards and SHA-256.
It works before configuration and while stopped/running without starting, probing,
reloading or writing the core/resources. Reports return counts and diagnostics,
not domains, IP addresses, group lists or attribute values. MMDB JSON metadata
retains its original numeric fields; DAT omits those fields and adds the `dat`
aggregate object. Invalid DAT returns the existing validation error envelope.

A structural pass deliberately does not claim Go regexp compilation, selected
matcher validity, category existence or classification/attribute-filter semantics.
`core_matching_verified` is always false. CN presence is reported independently;
missing CN warns that Mihomo initialization can delete/re-download the file when
its CN verification fails. Unknown-field warnings take precedence. Generated
fixtures include CN and disable external downloads, so the real-core check does
not exercise uncontrolled Geo initialization. Primary schema/loading/init source
references and the original service adaptation are recorded in `docs/UPSTREAM.md`.

The Web resource panel exposes both DAT checks, structural/unknown-field status,
family/regex/attribute/empty-group counts, CN absence and the fingerprint. Checking,
failure, retry, inventory refresh, lifecycle/revision changes and disconnect reuse
the existing stale-result guards. Downloads and installation are not added here.
`docs/RUNNING.md` documents the API, bounds, warnings and real-core check.

Verification:

- `cargo check --workspace`, formatting and the Web production build succeed.
  Workspace tests report **370 passed, 82 opt-in ignored**. Pure validation cases
  cover defaults, malformed known fields, truncation, varint overflow, field/group/
  text bounds, unknown fields, empty groups and compatibility limits. Snapshot
  checks cover digest/report privacy and symlink/FIFO/empty/oversized rejection;
  management checks cover authentication, both DAT names, corrupt-file errors and
  unchanged files/core revision.
- The explicitly enabled real-core DAT workflow passes with
  `/usr/bin/verge-mihomo`: standard/memconservative loaders × mph/succinct matchers
  each route exact, suffix, keyword, regexp, IPv4, IPv6 and unmatched requests to
  deterministic local HTTP proxy fixtures. It verifies read-only checks while
  running/stopped, malformed-file failure without core/revision changes, and
  restored operation after service restart. These fixtures do not prove arbitrary
  user DAT files compatible or change the API compatibility flag.
- The actual-node resource workflow passes against private copies of `data`
  subscriptions and Geo resources using `wlo1`: HTTPS proxy traffic returns 204
  after core restart and original source hashes remain unchanged.
- Full Chromium regression reports **28 passed, 4 optional bundle upgrade/repair
  workflows skipped**. The new DAT workflow verifies counts/fingerprints, missing
  CN, unknown-field warnings, failure/retry and refresh clearing; existing settings,
  profiles and MMDB workflows remain covered.

The complete architecture tree above is synchronized and the Linux MVP remains
runnable. Next: P1 stopped-core pinned DAT installation with compatibility checks
and recovery. Controlled online/running-core Geo lifecycle, full settings and
native TUN remain pending P1; P2 rules/provider/delay, P3 i18n/signals and P4 actual
systemd installation remain incomplete. Deferred work stays deferred. Git submission
is left to the external host script.

## Increment: stopped-core pinned DAT installation (P1)

Completed the next P1 task in Delivery order. The existing bundle manifest already
pins `geoip.dat` and `geosite.dat` by size and SHA-256. `geo_seed` now inspects
those two names; authenticated `install_geo_seed` accepts them only for a stopped,
reaped core and requires fresh seed/current digests. The bounded, no-follow copy
and private `.geo-seed` staging used for MMDB remain the publication path. Unknown
fields, missing CN, empty groups, unsafe group delimiters, malformed known fields,
wrong pins and `accept_metadata_only: true` block DAT installation before any
publication. MMDB metadata-only acceptance remains unchanged and cannot bypass
DAT checks.

DAT candidates also require a disposable private Mihomo `-t` probe. The probe
contains only the pinned DAT copy, disabled external Geo URLs and rules for every
group. It loads the rules under standard/memconservative loaders and mph/succinct
matchers. Each command has the existing bounded output/cancellation/reap behavior
and a 15-second limit. Only after all four succeed does the actor mark a private
core-load proof and atomically publish the staged file under a renewed current-file
digest guard. The receipt adds `core_load_verified: true` for DAT; MMDB receipts
retain their previous shape. `validation.dat.core_matching_verified` remains false:
loading every group does not prove individual record classification, attribute
filtering or behavior under a later changed core/configuration. Probe and staging
cleanup run on success/failure; fixed staging orphans are recovered at startup.
Publication does not restart the core or change the runtime revision.

The Web resource panel now offers DAT bundle inspection and stopped-core install.
It hides the MMDB-only metadata checkbox, prevents install while running, requires
fresh inspection after failure/phase change, and shows the explicit core-load proof
and fingerprint. Existing MMDB actions remain available. `docs/RUNNING.md` documents
the command, receipt, limits and reinspection rule; `docs/UPSTREAM.md` records
provenance and compatibility boundaries.

Verification:

- Workspace compilation, formatting and Web production build succeed. Workspace
  tests report **371 passed, 84 opt-in ignored**, including new DAT staging guards,
  simulated abandoned-DAT staging recovery and unchanged MMDB replacement checks.
  An unrelated core-upgrade test hit a
  transient `Text file busy` race under parallel execution; its isolated rerun
  and the complete serial workspace run pass.
- Three explicitly enabled real-core DAT workflows pass with
  `/usr/bin/verge-mihomo`: rule routing across loader/matcher combinations,
  digest-guarded stopped-core installation and post-install startup, and rejection
  of a structurally valid DAT with an invalid Go regexp without changing the old
  resource. Running-core installation is rejected.
- Full Chromium regression reports **29 passed, 4 optional bundle upgrade/repair
  workflows skipped**. It exercises DAT inspection, stopped-state gating,
  failed-probe retry, receipt proof and unchanged MMDB controls. The actual-node
  resource workflow checks proxy HTTPS 204 from private `data` copies and
  unchanged original hashes.

The complete architecture tree above is synchronized and the Linux MVP remains
runnable. Next: P1 controlled online Geo updates with bounded downloads, candidate
validation and stopped/running-core recovery. Remaining full settings/native TUN
are P1; P2 rules/provider/delay, P3 i18n/signals and P4 actual systemd installation
remain incomplete. Deferred work stays deferred. Git submission is left to the
external host script.

## Increment: stopped-core online Geo updates from committed sources (P1)

Completed the next P1 slice of controlled Geo lifecycle. Authenticated
`geo_online_info` reads an explicitly committed `geox-url` leaf for one allowlisted
Geo filename, returning only its source SHA-256 fingerprint and a confined current
file SHA-256 (or null). No URL, query string, credentials, path override or arbitrary
source is returned or accepted by the update command. `update_geo_online` requires
a stopped/reaped core plus both inspected fingerprints; an optional download SHA-256
pins the body. A source/config or current-file change requires reinspection.

`geo_online.rs` fetches directly without ambient HTTP proxy, redirects or URL
credentials, with 10-second connection and 20-second request limits. It streams at
most 128 MiB to a private scratch file with SHA-256; HTTP errors, empty bodies,
oversize content and mismatched optional pins fail before staging. The actor then
reuses no-follow bounded `.geo-seed` copy, MMDB strict/explicit metadata-only
validation or DAT known-schema/CN/nonempty checks, four Mihomo loader/matcher
probes, final current-file digest guard and atomic publication. The response uses
the existing receipt, including `core_load_verified: true` for DAT. Downloads and
probes are cancellable on service shutdown, and a failed update leaves the old
resource, process state and runtime revision unchanged. The fixed staging namespace
retains startup orphan recovery. The update never starts or reloads the core.

The Web resource panel provides separate online source inspection and explicit
stopped-core update controls for the five supported filenames, including optional
body digest input. A failed/ambiguous update discards its inspected snapshot and
requires a fresh read. The MMDB metadata-only choice remains separate from DAT
load verification. `docs/RUNNING.md` records the API and operational limits;
`docs/UPSTREAM.md` records provenance.

Verification:

- Workspace compilation and Web production build succeed. Workspace tests report
  **374 passed, 85 opt-in ignored**. Unit tests cover committed-source
  allowlists/privacy, direct bounded fetch, redirect/size/pin rejection and
  downloaded MMDB publication with stale-current protection. The authenticated
  management route rejects unauthorized or path/URL-extended online commands.
- An explicitly enabled real-core workflow serves local GeoIP/GeoSite DAT bytes,
  rejects stale source and body hashes, malformed data and invalid Go regexp while
  preserving the old file, then installs valid files, starts Mihomo and rejects an
  online update while it runs. Existing pinned-bundle DAT checks remain covered.
- Full Chromium regression reports **30 passed, 4 optional bundle upgrade/repair
  workflows skipped**. It covers source inspection, stopped-phase gating, local
  digest validation, failed update/reinspection and receipt display. The existing
  actual-node HTTPS 204 check uses private `data` copies and leaves originals
  unchanged.

The complete architecture tree above is synchronized and the Linux MVP remains
runnable. Next: P1 running-core Geo replacement with verified activation, rollback
and interruption recovery. Proxy-aware Geo download routing, remaining full
settings/native TUN remain P1; P2 rules/
provider/delay, P3 i18n/signals and P4 actual systemd installation remain
incomplete. Deferred work stays deferred. Git submission is left to the external
host script.

## Increment: running-core online Geo replacement and recovery (P1)

`update_geo_online` now accepts a settled running core as well as a stopped one.
It retains committed-source/current-file fingerprints, bounded direct download,
MMDB validation and four DAT loader/matcher probes. The running path finishes
candidate validation before interrupting traffic. Changed bytes create a private
`.geo-live` rollback record with a private previous-file copy and durable pending
marker; the actor then stops and reaps Mihomo, atomically publishes the candidate,
restarts the core, verifies readiness/listener readback, a second health response
and the installed file digest, and removes the marker as the commit point. Identical bytes skip the
restart. Failed publication or activation reaps the candidate, restores the old
file and restarts the prior core. If recovery cannot safely confirm the file hash,
it leaves the journal intact and blocks startup rather than overwriting an
unexpected external change. Startup recovers an interrupted pending transaction
under the data lock before optional bundle Geo seeding.

The Web action remains mounted while the core transitions through stopping and
starting, so a successful live update can display its receipt. It allows only
settled running/stopped phases and explains the brief proxy interruption. The
resource inventory refresh no longer discards the current panel during a status
transition. `docs/RUNNING.md` describes live and stopped semantics and the
remaining rule-level verification limit.

Verification:

- Workspace compilation and Web production build succeed. Workspace tests report
  **377 passed, 85 opt-in ignored**, including three journal rollback/commit and
  external-change protection cases.
- An explicitly enabled real Mihomo test exercises a running GeoSite DAT update,
  a candidate whose groups conflict with the committed rule configuration,
  old-file/core rollback, and startup recovery of a simulated interrupted
  pending journal. Existing stopped-core source/digest and DAT checks remain in
  the same workflow.
- Full Chromium regression reports **30 passed, 4 optional bundle upgrade/repair
  workflows skipped**. It covers live phase changes without losing the receipt,
  required reinspection after success/failure, and the retained bundle-install
  behavior. The actual-node test uses private `data` profile copies and returns
  HTTPS 204 through a selected proxy; original node files remain untouched.

The complete architecture tree above is synchronized and the Linux MVP remains
runnable. Next: P1 proxy-aware Geo download routing and TLS retry parity with
subscription downloads. Remaining full settings/native TUN remain P1; P2 rules/
provider/delay, P3 i18n/signals and P4 actual systemd installation remain
incomplete. Deferred work stays deferred. Git submission is left to the external
host script.

## Increment: proxy-aware online Geo download and TLS retry parity (P1)

`update_geo_online` accepts an explicit `direct` (default), `system` or
`managed` route and an explicit `danger_accept_invalid_certs` flag. The source
still comes only from the committed `geox-url` leaf and is pinned by its
inspection digest; no caller URL, destination or arbitrary proxy endpoint is
accepted. System routing reuses validated service proxy environment/NO_PROXY
handling. Managed routing requires a running core, verifies its reported
HTTP/Mixed listener against committed configuration and resolves authentication
from that private configuration. A route error never silently falls back to a
different route. The actor confirms the core remains running after a managed
download before staging or stopping it for live publication.

Geo downloads now use the shared platform/static WebPKI root policy, legacy TLS
diagnostic and source-redacted transport errors used for subscriptions. A TLS
certificate failure makes one static-root retry on the same route within a
single 20-second total deadline; an explicit certificate exception disables
verification and retry. The existing no-redirect, 128 MiB streamed-body cap,
SHA-256 pin, private staging, MMDB/DAT checks and live rollback are preserved.
The Web action exposes route and certificate choices without rendering the
source URL or credentials. `docs/RUNNING.md` records route semantics and limits.

Verification:

- Workspace compilation and Web production build succeed. The workspace reports
  **379 passed, 85 opt-in ignored**. It covers a process-isolated system proxy route against a Geo-only URL,
  keeping direct as the default and rejecting managed while stopped. A private
  self-signed HTTPS fixture confirms two verified-root attempts, one explicit
  certificate-exception attempt, URL redaction and unchanged old Geo data.
- The opt-in real Mihomo workflow updates a GeoSite DAT through the live managed
  HTTP/Mixed listener and retains the previous live activation/rollback/startup
  recovery checks. The actual-node proxy workflow returns HTTPS 204 through a
  selected node using private copies of `data` and leaves source subscriptions
  untouched.
- Full Chromium regression reports **30 passed, 4 optional bundle upgrade/repair
  workflows skipped**. The online Geo action covers explicit managed-route and
  certificate choices while preserving its inspected hashes through live phase
  changes and requiring fresh inspection after a failed or successful update.

The complete architecture tree above is synchronized and the Linux MVP remains
runnable. Next: P1 remaining authoritative settings and native Linux TUN
integration. P2 rules/provider/delay, P3 i18n/signals and P4 actual systemd
installation remain incomplete. Deferred work stays deferred. Git submission is
left to the external host script.

## Current increment: localized service messages and error responses

Delivery step 10 (P3) connects HTTP and WebSocket management error responses to
the embedded `clash-verge-i18n` localization engine using `Accept-Language` headers:
- **`clash-verge-i18n` catalog and resolution**:
  - `resolve_accept_language(header_value)` parses weighted quality factors (RFC 9110)
    and resolves the client's highest-preference language tag to an embedded locale.
  - `translate_service_error(code, default_message, language)` looks up error codes
    in `service.errors.<code_snake_case>` and `service.errors.<code_camel_case>`,
    returning localized strings for `zh`, `zhtw`, and `en` while preserving verbatim
    `default_message` when no language is requested or when untranslated.
- **Service HTTP error handling (`service/src/management/http.rs`)**:
  - `language_from_headers`, `error_with_headers`, and `error_with_language` resolve
    the caller's preferred language tag from `Accept-Language`.
  - Authentication, command parsing, method-not-allowed fallback, backup import/export,
    restore validation, and web asset/streaming endpoints return localized error bodies
    (`{"error": {"code": code, "message": localized}}`).
- **Web client integration (`web/src/api.ts`)**:
  - `command()` passes `Accept-Language: savedLanguage()` in JSON POST headers so
    the server's error responses reflect the browser's currently chosen UI language (`zh` or `en`).
  - Fallback error messages in `ApiError` adapt to the active client language.

Verification: `cargo test -p clash-verge-i18n` passes all 7 unit tests (including
weighted `Accept-Language` resolution and error translation fallbacks). `cargo test
--test management http_errors_localize_via_accept_language_and_preserve_default_fallback`
verifies Chinese, English, and unlocalized fallback behaviors across 404, 401, 405, 400,
and 503 error states. Full management test suite (23 passed, 1 ignored) and Web
production build pass cleanly. All 49 Playwright UI workflow tests pass (4 skipped).
Next: additional browser languages and service signals verification.

## Previous increment: resource views and Geo action localization

Delivery step 9 (P3) now localizes the runtime resource inventory panel
(`ResourcesPanel`), offline seed installation controls (`GeoSeedAction`), and
online Geo download controls (`GeoOnlineAction`): section titles, refresh
actions, data/bundle directory prefixes, Geo auto-update policy statuses
(active, disabled, stopped, indeterminate), core effective readback, freshness
badges (fresh, stale, indeterminate), file metadata labels (file exists,
missing, empty, unreadable, etc.), relative file modification timestamps,
MMDB/DAT structural validation messages with detailed group/record/attribute
counts and CN group detection, seed candidate and current file fingerprints,
download route options (direct, system proxy, managed proxy), certificate
verification warnings, empty-description MMDB installation allowances, in-flight
progress indicators, and detailed completion receipts. All dynamic placeholders
interpolate via `t()`. Switching between Chinese and English preserves loaded
inventory, open seed/online cards, and in-flight operations without unmounting
or triggering redundant requests.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow navigates to the settings page, mocks resource inventory
and seed/online endpoints, verifies Chinese labels, validation buttons, and
expanded seed/online forms, switches to English and confirms all translated
headers, policy statuses, file state tags, action buttons, and route dropdowns,
and switches back to Chinese. The full browser regression test suite reports
**49 passed and 4 optional upgrade workflows skipped**. The complete architecture
tree above is synchronized. Next: translate service-message errors and
notifications, followed by additional browser languages and signals verification.

## Previous increment: core upgrade view localization

Delivery step 9 (P3) now localizes the core upgrade panel (`CoreUpgradePage`):
channel selector (Stable / Alpha), panel titles, install info refresh action,
description copy, pre-release channel notice, disconnected notice, installed
version labels (installed version, latest release version, install record,
unverified record, bundle initialization, and repair-needed guidance), action
buttons (check updates, upgrade, force reinstall with confirmation dialog),
working/progress indicators, upgrade and repair report banners, mismatch
warnings, and operational footer hints. Dynamic placeholders for channel label
(`{label}`), versions (`{version}`, `{from}`, `{to}`), and error messages
(`{message}`) interpolate cleanly across language changes via parameter support
in `t()`. Switching between Chinese and English preserves selected channels,
active installation reads, and in-flight states without unmounting or duplicate
requests.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow navigates to the core upgrade page, mocks core version and
release endpoints, verifies Chinese labels and controls in the default Stable
channel, switches to English and confirms all translated labels and action buttons,
switches channel to Alpha and verifies the pre-release notice and channel-specific
actions, switches back to Chinese to verify localized Alpha labels, and returns
to Stable channel. The full browser regression test suite reports **48 passed and
4 optional upgrade workflows skipped**. The complete architecture tree above is
synchronized. Next: translate resource views (Geo and provider management panels).

## Previous increment: logs view and filter localization

Delivery step 9 (P3) now localizes the core logs panel (`LogPage`) and log line
container (`LogLines`): section heading, update/reconnect subtitle, filter input
aria-label and placeholder, filter clear action, log region accessibility labels,
initial empty state, and empty search/filter result notices. Mihomo stream badges
(`stdout`, `stderr`) and log payload messages remain raw core outputs. Changing
browser language retains active filter input and visible filtered entries without
re-fetching or interrupting realtime log events.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow navigates to the logs page, tests Chinese controls, filters
by active log content, switches to English, verifies translated labels and preserved
filter text, tests an unmatched filter string with empty-result feedback, switches
back to Chinese to verify translated empty feedback, and clears the filter using
the clear button. The full browser regression test suite reports **47 passed and
4 optional upgrade workflows skipped**. The complete architecture tree above is
synchronized. Next: translate resource and upgrade views.

## Previous increment: rule-provider inventory and update localization

Delivery step 9 (P3) now localizes the external rule-provider panel on the rules
page: section title, subtitle, update-all button, provider card format/type labels,
rule count badge, timestamp label, update action, pending "Updating…" / "更新中…"
states, and operation feedback notices (single provider update completion, all
providers updated, and localized update errors). Provider names, behavior tags,
format tokens, and timestamp values retain their core values. In-flight update
button states and status messages translate dynamically upon switching language
without cancelling or duplicating requests.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow supplies a deterministic rule provider, tests Chinese labels,
switches to English, initiates an update, switches languages while the update is
pending, verifies re-enabled actions, tests localized completion notice, and
exercises the update-all command. The browser test suite passes with **46 passed
and 4 optional upgrade workflows skipped**. The complete architecture tree above
is synchronized. Next: translate the logs view controls and status indicators.

## Previous increment: rule list and search localization

Delivery step 9 (P3) now localizes the rules page heading and count summary,
refresh/stopped guidance, rule-list region, search and clear controls, match
count, empty/loading notices and table headers. Rule types, payloads and target
names remain core-supplied values. Changing browser language keeps the current
search input and filtered rows without fetching a new rules snapshot. The
rule-provider panel on this page remains untranslated P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow supplies two deterministic rules, filters them, switches
languages, checks table and empty-result translations, then clears the filter
without refetching rules. The complete architecture tree above is synchronized.
Next: translate rule-provider inventory and update feedback.

## Previous increment: proxy-provider inventory and actions localization

Delivery step 9 (P3) now localizes the proxy-provider panel's title, collection
and node counts, update-all and per-provider update/healthcheck actions, and
their in-progress labels. Provider names, vehicle types and timestamps retain
their source values. Changing browser language during an in-flight update
relabels its pending action without changing the request. The authenticated
provider operations and refresh behavior remain unchanged.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow supplies a provider response, holds one update while
switching languages, then exercises individual update, healthcheck and
update-all commands. The full browser regression reports **44 passed and 4
optional upgrade/repair workflows skipped**. The complete architecture tree
above is synchronized. Next: translate the rules page's rule list, search and
table controls.

## Previous increment: proxy delay-test localization

Delivery step 9 (P3) now localizes the proxy page's delay-test URL label and
placeholder, group test button, per-node test hint and testing/untested/timeout
badges. Numeric latency stays in milliseconds. Browser language changes retain
the entered test URL and previous results; node and group testing still use the
existing authenticated commands and timeout policy. Provider controls on the
same page remain untranslated P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow starts the fixture core, enters a custom URL, switches
languages and uses deterministic node/group delay responses to check the URL,
numeric results and translated timeout badge. The full browser regression
reports **43 passed and 4 optional upgrade/repair workflows skipped**. The
complete architecture tree above is synchronized. Next: translate
proxy-provider inventory and update/healthcheck controls.

## Previous increment: proxy node list and selection localization

Delivery step 9 (P3) now localizes the proxy page's selection guidance,
refresh/loading notices, group current value, unfix control, node selection
labels and empty-group notice. Dynamic proxy and group names remain unchanged.
Switching browser languages keeps the active selection; selecting another node
still uses the existing authenticated command and persists per profile. Delay
tests and provider controls on the same page remain untranslated P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow starts the fixture core, confirms its selected node,
switches to English, changes selection, switches back to Chinese and restores
the original selection. The full browser regression reports **42 passed and 4
optional upgrade/repair workflows skipped**. The complete architecture tree
above is synchronized. Next: translate proxy delay-test controls and badges.

## Previous increment: global script editor localization

Delivery step 9 (P3) now localizes the global script extension editor's
instructions, JavaScript field, byte-limit feedback, save/cancel controls and
default-reset confirmation. Browser language changes preserve the mounted
editor, unsaved source, size feedback and pending confirmation. The existing
authenticated read/save/reset commands and script validation are unchanged.
Proxy, rules, log, resource and upgrade views still need P3 translation.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow switches languages with an unsaved source and open reset
confirmation, checks translated oversized-source feedback, then saves, reads
back and restores the original default script through authenticated commands
without changing core generation. The full browser regression reports **41
passed and 4 optional upgrade/repair workflows skipped**. The complete
architecture tree above is synchronized. Next: translate the proxy page's
node list and selection controls.

## Previous increment: global merge editor localization

Delivery step 9 (P3) now localizes the global merge extension panel guidance,
YAML field, size feedback, save/cancel controls and default-reset confirmation.
Browser language changes preserve the mounted editor, unsaved YAML and pending
confirmation. The existing authenticated read/save/reset commands and active
profile application behavior are unchanged. The global script editor's
specific controls and the remaining management views still need P3 translation.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow switches languages with an unsaved YAML draft and open
reset confirmation, then saves, reads back and restores the original default
merge through authenticated commands without changing core generation. The
full browser regression reports **40 passed and 4 optional upgrade/repair
workflows skipped**. The complete architecture tree above is synchronized.
Next: translate the global script extension editor.

## Previous increment: profile-linked script editor localization

Delivery step 9 (P3) now localizes the profile-linked script editor's title,
profile-specific execution and failure guidance, JavaScript field and
save/remove/cancel controls. Browser language changes preserve the mounted
editor and unsaved source. The existing authenticated set/clear commands and
script validation behavior are unchanged. Global merge/script editors and
other management views remain P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow imports an inactive local profile, edits JavaScript,
switches to English, saves and reads back the source, removes its script link,
checks unchanged core generation and deletes the fixture. The full browser
regression reports **39 passed and 4 optional upgrade/repair workflows
skipped**. The complete architecture tree above is synchronized. Next:
translate the global merge extension editor.

## Previous increment: profile-linked sequence editor localization

Delivery step 9 (P3) now localizes the profile-linked sequence editor's title,
profile-specific instructions, rules/proxies/groups choices, YAML field and
save/remove/cancel controls. Browser language changes preserve the mounted
editor, its selected kind and unsaved YAML. Choosing a different sequence kind
still fetches that kind and discards the prior unsaved draft, as the existing
instructions state. The authenticated set/clear/readback flow is unchanged;
profile-linked script and global extension editors remain pending P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow imports an inactive local profile, edits rules YAML,
switches to English, saves and reads back the rules, changes type through the
localized selector, then removes only the rules link. Other default sequence
links remain as expected. The workflow verifies unchanged core generation and
deletes the fixture. The full browser regression reports **38 passed and 4
optional upgrade/repair workflows skipped**. The complete architecture tree
above is synchronized. Next: translate the profile-linked script editor.

## Previous increment: profile-linked merge editor localization

Delivery step 9 (P3) now localizes the profile-linked merge editor's title,
profile-specific instructions, YAML field and save/remove/cancel controls. The
browser-owned language change preserves the editor component and unsaved YAML.
The existing authenticated set/clear commands and active-versus-inactive profile
behavior are unchanged. Profile-linked sequence/script and global extension
editors still contain Chinese-only copy.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow imports an inactive local profile, edits merge YAML,
switches to English, saves and reads back the linked content, removes it, and
confirms unchanged core generation before deleting the fixture. The full
browser regression reports **37 passed and 4 optional upgrade/repair workflows
skipped**. The complete architecture tree above is synchronized. Next:
translate the profile-linked sequence editor.

## Previous increment: raw subscription YAML editor localization

Delivery step 9 (P3) now localizes the raw editor's instructions, controls,
accessible names, conflict and uncertain-state warnings, and read/verify/save
feedback. Feedback is stored by meaning and rendered in the selected browser
language; server-provided error details remain verbatim. Invalid raw responses
use a translatable local error. The current language also determines the
authentication-expiry message. Changing language does not trigger a fresh read,
replace an unsaved draft, clear a confirmation or bypass revision/conflict
guards. Profile extension editors and other management views remain P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow injects a failed raw read, changes language while the error
is shown, retries, edits a draft, translates the reload confirmation and
verification result in place, then discards the draft and deletes its fixture.
It confirms unchanged core generation. The full browser regression reports
**36 passed and 4 optional upgrade/repair workflows skipped**. The complete
architecture tree above is synchronized. Next: translate the profile-linked
merge editor.

## Previous increment: profile metadata editor localization

Delivery step 9 (P3) now localizes the profile metadata editor's title,
instructions, name and description, remote URL, User-Agent, timeout and update
interval, automatic update and proxy/TLS choices, and save/cancel actions. It
uses the browser-owned Chinese/English catalog while retaining the existing
patch-only authenticated save. Changing language keeps field values and checkbox
states without remounting the editor or changing service generation. Raw YAML
and profile extension editors remain Chinese-only P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow imports a remote fixture, edits its metadata and download
options, switches to English, saves the draft, verifies the service profile
readback and unchanged core generation, then deletes the fixture. The full
browser regression reports **35 passed and 4 optional upgrade/repair workflows
skipped**. The complete architecture tree above is synchronized. Next:
translate the raw subscription YAML editor.

## Previous increment: subscription import form localization

Delivery step 9 (P3) now localizes both profile import forms: titles, help,
field labels, download-route and TLS choices, and submit actions use the
browser-owned Chinese/English catalog. The local file-size error is stored as a
meaningful state and rendered in the selected language, while other file-read
and server errors remain raw. Language changes retain local YAML and name,
remote URL and name, and route/TLS checkbox choices without writing service
settings. The import commands still use their existing authenticated service
flow; profile metadata, raw and extension editors remain separate P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow checks the translated 8 MiB limit, switches language while
drafts and TLS choice are set, imports local and remote fixtures through the
English forms, and deletes both profiles. It also confirms unchanged service
generation before importing. The full browser regression reports **34 passed
and 4 optional upgrade/repair workflows skipped**. The complete architecture
tree above is synchronized. Next: translate the profile metadata editor.

## Previous increment: profile list localization

Delivery step 9 (P3) now localizes the profile list's empty state, count,
local/remote type, linked-enhancement and usage labels, active badge, actions,
accessible action names, and deletion confirmation through the browser-owned
Chinese/English catalog. The selected browser language changes these labels
without remounting the page, clearing its pending deletion state or modifying
service settings. The import forms, profile detail/extension editors and other
management views still contain Chinese-only copy and remain separate P3 work.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow creates a temporary local profile, checks English list and
action labels, switches back to Chinese while deletion is pending, deletes the
fixture and checks unchanged service generation. The full browser regression
reports **33 passed and 4 optional upgrade/repair workflows skipped**. The
complete architecture tree above is synchronized. Next: translate local YAML
and remote URL subscription import forms.

## Previous increment: runtime configuration editor localization

Delivery step 9 (P3) now localizes the runtime configuration editor's title,
YAML field, validation action, instructions, unsaved-change status and local
success/missing-configuration feedback through the browser-owned Chinese/English
catalog. Feedback is stored by meaning and rendered in the selected language,
so a language change updates an existing message. The editor component remains
mounted across language changes and retains an unapplied YAML draft. Raw core
validation errors are still shown as returned by the service; service-message
localization is a separate pending P3 task.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright workflow switches English to Chinese with an unsaved YAML draft,
checks translated missing-configuration feedback and confirms unchanged service
generation. The full browser regression reports **32 passed and 4 optional
upgrade/repair workflows skipped**. The complete architecture tree above is
synchronized. Next: translate the profile and subscription management workflows.

## Previous increment: browser-owned language selection and management shell

Delivery step 9 (P3) now has a browser-local Chinese/English language setting.
The selector appears before login and in the management sidebar. It follows the
upstream Chinese fallback and regional language normalization, persists only in
the browser origin's local storage, and updates the document language/title.
Login, navigation, connection and core status, global start/stop/restart
controls, feedback, and the overview's main labels use a typed message catalog.
Changing language does not write service settings or recreate the WebSocket.
An isolated browser context starts with its own default language. The existing
Chinese UI and management behavior remain the default; detailed settings,
profile, proxy, rules, log, resource and upgrade pages still need translation.
The initial browser catalog covers Chinese and English; other upstream browser
locales remain pending, as do localized service messages.

Verification: `cargo check --workspace` and the Web production build pass. A
Playwright test covers login and navigation switching, persistence after reload,
isolated browser storage, unchanged service generation and token absence from
local storage. The full browser regression reports **31 passed and 4 optional
upgrade/repair workflows skipped**. The complete architecture tree above is
synchronized. Next: translate the remaining
P3 management views, starting with configuration and profile workflows.

## Previous increment: shared locale catalog and Linux service signals

Delivery step 9 (P3) begins with the in-flight upstream shared-component
extraction. `clash-verge-i18n` embeds the thirteen pinned YAML locale files,
retains upstream aliases and fallback behavior, and offers an explicit
`translate_for` lookup that does not change the process-wide backend locale.
Embedded locale parse errors now fail visibly rather than silently creating an
empty catalog. The browser does not yet consume this catalog or offer language
selection. `clash-verge-signal` retains the upstream shutdown latch and Unix
SIGTERM/SIGINT/SIGHUP selection, using the existing Tokio runtime. The foreground
service consumes that listener and retains its one supervisor, HTTP/WebSocket
drain and managed Mihomo child reaping. No desktop event loop or second runtime
is introduced; non-Linux signal adaptation remains deferred.

Verification: offline `cargo check --workspace` and the serial workspace suite
pass with **400 passed, 89 opt-in ignored and zero failures**. All embedded
locale files parse, and explicit English/Chinese lookups do not mutate backend
locale. A real-Mihomo process test confirms
SIGTERM, SIGINT and SIGHUP exit successfully, reap the managed child, and keep
the management service alive after a failed core start. The complete tree above
is synchronized; the Linux MVP remains runnable. Next: browser-owned language
selection and translation of the management UI, followed by service-message
localization and P4 systemd installation. P3 is partial.

## Previous increment: proxy providers and latency delay testing

Delivery step 8 (P2) completes proxy providers and delay testing for the managed
service and Web interface:
- `headless-core` defines `DelayTestQuery` and re-exports provider operations in
  `config::operations`.
- `CoreManager` implements `proxy_providers()`, `update_proxy_provider(name)`,
  `healthcheck_proxy_provider(name)`, `delay_proxy(name, url, timeout)` and
  `delay_group(group, url, timeout)` with `CorePhase::Running` checks, bounded
  deadlines, and safe JSON mapping.
- `ManagementCommand` registers `ProxyProviders`, `UpdateProxyProvider`,
  `HealthcheckProxyProvider`, `DelayProxy`, and `DelayGroup` with bearer-token
  authentication and 422 Unprocessable Entity error reporting when the core is
  stopped or the target does not exist.
- The Web proxies view integrates live delay testing per group and per individual
  node, color-coded latency badges (fast `<300ms`, medium `<600ms`, slow, timeout,
  untested), a customizable delay test URL input bar, and an external Proxy
  Providers panel with per-provider update and healthcheck actions as well as
  batch update-all.

Verification: `cargo check --workspace` and the serial workspace test suite pass
cleanly. Management commands reject unauthenticated and stopped-core requests
with 422 Unprocessable Entity (`service/tests/management.rs`). Live integration
test `service/tests/delay_live.rs` validates proxy provider query, update,
healthcheck, group delay testing, and node delay testing against
`/usr/bin/verge-mihomo`. The actual-node proxy workflow in
`service/tests/resource_inventory_live.rs` returns HTTPS 204 through private
copies of `data`. The Playwright browser test suite reports **30 passed, 4
optional upgrade/repair workflows skipped**.

Next: P3 i18n and service signals (migrate language resources and Unix signal behavior).

## Previous increment: rules and rule-provider management

Delivery step 8 (P2) now exposes authenticated query and update operations for
Mihomo routing rules and external rule providers:
- `headless-core` defines `ProviderAction` (`Update`, `Healthcheck`) and `ProviderOperationReceipt`
  in `config::operations`.
- `CoreManager` implements `rules()`, `rule_providers()` and `update_rule_provider(name)`
  with `CorePhase::Running` checks, bounded timeouts (10s for reads, 30s for updates),
  and safe JSON mapping from the underlying Mihomo controller client.
- `ManagementCommand` registers `Rules`, `RuleProviders`, and `UpdateRuleProvider`
  with token authentication and 422 Unprocessable Entity error reporting when the
  core is stopped or the provider does not exist.
- The Web UI adds a dedicated Rules page (`05 规则` at `/rules`) featuring rule count
  metrics, search and filter bar (by rule type, payload, or target proxy), styled badge
  visualization, and an external Rule Providers panel with per-provider update and
  update-all actions.

Verification: `cargo check --workspace` and the serial workspace suite pass with
zero failures. Management commands reject unauthenticated requests and stopped-core
invocations with 422 Unprocessable Entity (`service/tests/management.rs`). Real-Mihomo
live test `rules_live.rs` validates rule listing, rule provider readback, and provider
updates against `/usr/bin/verge-mihomo`. The actual-node proxy workflow in
`service/tests/resource_inventory_live.rs` returns HTTPS 204 through private copies
of `data`. The Playwright browser test suite reports **30 passed, 4 optional upgrade/repair
workflows skipped**.

Next: P2 proxy providers and delay testing (`delay_group`, `delay_proxy`,
`healthcheck_proxy_provider`, latency badges in Web proxies view).

## Previous increment: effective automatic-update state and resource freshness

Delivery step 7 now inspects and exposes the effective automatic-update state
(`Active`, `Disabled`, `Stopped`, `Indeterminate`) and resource freshness (`Fresh`,
`Stale`, `Indeterminate`) on the committed resource inventory without duplicating
Mihomo's background updater. `GeoUpdatePolicy` derives its runtime state by
combining core execution status, readback success, explicit/effective enable flags,
and zero/non-zero update intervals. When the core is stopped, auto-update is
marked `Stopped`; when readback fails, `Indeterminate`; when disabled or configured
with a zero interval, `Disabled`; and when running with a positive interval,
`Active`.

Each available Geo and provider file now evaluates its age against its effective
update interval (hours for Geo assets, seconds for HTTP providers). Resources
modified within the interval are marked `Fresh`; resources older than the interval
are marked `Stale`; and resources that are empty, missing, unmanaged, or have future
mtimes due to clock skew are marked `Indeterminate`. The Web resource panel displays
the auto-update running state alongside submitted and core policy readback, and renders
color-coded freshness diagnostics for available resources.

Verification: `cargo check --workspace` and the serial workspace suite pass with
**387 passed, 87 opt-in ignored and zero failures** (including a new targeted
freshness and interval evaluation test). Clippy with `-D warnings` and `cargo fmt`
pass cleanly. The real-Mihomo live resource inventory test (`resource_inventory_live`)
passes against private copies of actual `data` and verifies HTTPS 204 proxy traffic.
The Playwright browser suite reports **30 passed, 4 optional upgrade/repair workflows
skipped**. The complete architecture tree is synchronized. Next: privileged Linux
TUN interface/route/traffic verification when `/dev/net/tun` is available, followed
by remaining P1 resource validations before P2.

## Previous increment: presence-preserving Geo update-policy readback

Delivery step 7 now reads the nine Geo settings from a dedicated, optional-field
`GET /configs` projection instead of the legacy `BaseConfig` defaults. A core
that omits `geo-auto-update`, `geo-update-interval`, a Geo URL leaf or another
Geo field now reports that actual value as unknown, without an invented
`false`, `0` or empty string or a false mismatch. Explicit values remain
comparable. The projection accepts both the `geoip`/`geo-ip` and
`geosite`/`geo-site` URL spellings; a real Mihomo build returns `geo-site`.
The settings store, candidate generation and browser contract are unchanged.
Mihomo still owns the configured automatic update timer; the service's
resource validation and explicit online replacement remain separate.

Verification: `cargo check --workspace` and the serial workspace suite pass
with **386 passed, 87 opt-in ignored and zero failures**. Targeted partial-core
tests cover missing leaves and explicit false/zero/empty values. The opt-in
real-Mihomo Geo application, rollback and restart test passes, and the
actual-node workflow returns HTTPS 204 through private copies of `data`.
The complete architecture tree above is synchronized. The Linux MVP remains
runnable. Next: inspect and expose effective automatic Geo update state and
resource freshness, followed by remaining P1 resource settings and privileged
TUN route verification. P2–P4 and deferred work remain incomplete.

## P1 increment: remaining DNS page controls

Delivery step 7 now includes `fake-ip-filter-mode` (`blacklist`/`whitelist`),
`prefer-h3` and `respect-rules` in the existing DNS settings object. The two
switches retain the DNS page's established semantics: saved `true` overrides,
saved `false` inherits a subscription or enhancement value. Either nonempty
filter-mode enum value is authoritative. Invalid enum values and wrong boolean
types fail before settings publication. The fields use the existing staged
generation, Mihomo validation, live application, rollback and schema-one
settings recovery; no new on-disk version or command is introduced.

The Web DNS editor exposes all three fields and preserves whole-runtime
replacement, inherited values and failed drafts. A real Mihomo candidate
accepts the three controls together with the previously delivered resolver
policies and fallback filter. Upstream field values come from
`src/components/setting/mods/dns-viewer.tsx` and
`src-tauri/src/utils/init.rs` at the pinned revision; the service authority
and browser controls are headless adaptations.

Verification: `cargo check --workspace` passes; the serial workspace suite
reports **385 passed, 87 opt-in ignored and zero failures**. Targeted model,
management API, real-Mihomo application/rollback and browser editor tests pass.
The actual-node resource/proxy workflow still returns HTTPS 204 using private
copies of `data`. The Linux MVP remains runnable. Privileged native TUN routing
still cannot be verified on this host without `/dev/net/tun` and network
administration capability. Next: remaining P1 Geo resource update policy and
readback, followed by other resource settings and the host-dependent TUN test.

## Previous increment: DNS resolver policy and fallback-filter authority

Delivery step 7 extends the version-one service settings without changing its
on-disk schema. DNS now accepts direct/proxy resolver lists, the direct resolver
policy switch, nameserver and proxy-server policy maps, and typed fallback-filter
fields. Policy map values retain upstream scalar-or-list form and reject invalid
saved shapes. The saved DNS page continues to let false and empty top-level
values inherit, so a false direct resolver policy switch does not override a
true source value. A nonempty policy map owns that complete policy field;
an empty map inherits. Fallback-filter ownership is per leaf, including an
explicit `geoip: false`, so changing its domain list does not erase source
GeoIP code or CIDR entries. Runtime generation, late enhancement authority,
saved settings recovery and changed-field diagnostics share those semantics.

The Web DNS editor renders these fields as typed lists, switches and JSON
objects, rejects malformed policy/filter shapes before submission, and retains
the existing read/whole-runtime replacement and failed-draft behavior.
Upstream references are `src/components/setting/mods/dns-viewer.tsx` and
`src-tauri/src/config/dns.rs` at the pinned revision; the strict service
models and per-leaf fallback merge are new headless adaptations.

Verification: `cargo check --workspace` and the serial workspace suite pass
with **384 passed, 87 opt-in ignored and zero failures**. The authenticated
management settings round-trip and a real Mihomo policy/fallback application
with invalid-candidate rollback pass. The full browser suite reports **30
passed, 4 optional upgrade/repair workflows skipped**. A private
copy of the actual `data` nodes again carries HTTPS 204 proxy traffic. This
host still lacks `/dev/net/tun` and effective network
administration capability, so privileged TUN routing remains unverified.
Next: remaining DNS page controls and resource settings in P1; run the privileged
TUN interface/route/traffic workflow once an appropriate host is available.

## Previous increment: remaining authoritative settings (bind, auth, LAN ACL, TFO/MPTCP, sniffing)

Delivery step 7 (P1) completes the remaining authoritative runtime settings in `crates/headless-core`:
- **Inbound listener binding and ACL controls (`RuntimeSettings`, `settings.rs`)**:
  - `bind_address`: supports `*`, empty, loopback/localhost, and valid IPv4/IPv6 addresses, with length and control-character bounds.
  - `authentication`: strictly validated list of `username:password` credentials, requiring non-empty usernames.
  - `skip_auth_prefixes`, `lan_allowed_ips`, `lan_disallowed_ips`: validated lists of IPv4/IPv6 addresses or CIDR network blocks (with prefix length bounds <= 32 for IPv4 and <= 128 for IPv6).
  - Explicit empty lists `[]` own the setting and clear any source subscription credentials or ACL rules.
- **TCP optimization and sniffing options**:
  - `inbound_tfo`: boolean flag controlling TCP Fast Open on inbound proxy listeners.
  - `inbound_mptcp`: boolean flag controlling Multipath TCP on inbound listeners.
  - `sniffing`: boolean flag controlling domain and protocol sniffing.
  - Explicit `false` booleans have full authority over `true` in subscriptions; absent `None` preserves subscription inheritance.
- **Verification & core lifecycle**:
  - Unit tests in `headless-core` verify strict syntax validation, invalid input rejection, authority overrides, empty-list clearing, and change reporting via `overridden_fields`.
  - Integration test `remaining_authoritative_settings_apply_hot_reload_and_survive_restart` in `service/tests/settings.rs` verifies that real Mihomo accepts these settings, applies them via hot reload, rejects invalid CIDRs with clean rollback, and fully restores all settings across a cold service restart.

Verification: `cargo check --workspace` passes; `cargo test -p headless-core --test settings` passes all 24 unit tests; `cargo test -p mihomo-server --test settings` passes real Mihomo hot-reload and restoration; all 20 python tests in `scripts/tests` pass.
The tree above marks remaining authoritative settings as implemented and Linux verified.

## P1 increment: remaining Geo lifecycle and resource settings

Delivery step 7 (P1) completes the remaining Geo lifecycle and resource settings in `crates/headless-core`:
- **Geo asset path and namespace protection (`resource_paths.rs`)**:
  - Added `is_geo_asset` helper recognizing all six canonical Geo assets (`Country.mmdb`, `ASN.mmdb`, `geoip.dat`, `geosite.dat`, `geoip.metadb`, `GeoSite.dat`) case-insensitively.
  - `check_destination`: strictly enforces case-insensitive protection preventing provider declarations from targeting or overwriting Geo assets (e.g. `geoip.DAT`, `country.mmdb`, `GEOSITE.DAT`).
  - Provider cache namespace protection: local providers cannot claim the reserved `HTTP_CACHE_ROOT` ("provider-cache") under `prepare_owned`.
- **Geo lifecycle models & key mapping (`settings/geo.rs`, `settings.rs`)**:
  - `GeoUrls::key_for_asset` and `GeoUrls::asset_for_key`: canonical symmetric bidirectional mapping between Geo asset filenames and `geox-url` configuration keys (`geoip.dat` <-> `geoip`, `geosite.dat`/`GeoSite.dat` <-> `geosite`, `Country.mmdb`/`geoip.metadb` <-> `mmdb`, `ASN.mmdb` <-> `asn`).
  - `GeoUrls::url_for_asset`: resolves configured committed URL for any Geo asset filename.
  - `GeoUrls::is_empty`: evaluates whether all URL leaves are unset.
  - `RuntimeSettings::expected_geo_assets`: derives expected database assets based on configured `geodata_mode` (DAT assets `["geoip.dat", "geosite.dat"]` for `true`, MMDB assets `["Country.mmdb", "ASN.mmdb", "geoip.metadb"]` for `false`, `None` for inheritance).
  - `service/src/geo_online.rs`: integrated with `GeoUrls::key_for_asset` for consistent source key extraction.
- **Verification & core lifecycle**:
  - Unit tests in `headless-core`: `tests/resource_paths.rs` tests case-insensitive Geo asset protection and `provider-cache` namespace isolation; `tests/settings.rs` tests `GeoUrls` lookup helpers, `expected_geo_assets` mode derivation, and `overridden_fields` detection for all 9 Geo configuration fields.
  - Integration test in `service/tests/settings.rs`: `geo_lifecycle_settings_apply_readback_and_survive_restart` verifies real Mihomo core hot-reload, `geo_settings` snapshot readback, invalid interval/URL rejection and rollback, and full restoration across service restarts.

Verification: `cargo check --workspace` passes; `cargo test -p headless-core --test resource_paths` passes all 9 unit tests; `cargo test -p headless-core --test settings` passes all 25 unit tests; `cargo test -p mihomo-server --test settings geo_lifecycle_settings_apply_readback_and_survive_restart -- --ignored` passes against real Mihomo; `cargo clippy --workspace --all-targets -- -D warnings` passes cleanly; all 20 python tests in `scripts/tests` pass.
The tree above marks remaining Geo lifecycle and resource settings as implemented and Linux verified.

## P1 increment: full resource settings

Delivery step 7 (P1) completes the full resource settings and models in `crates/headless-core`:
- **Unified resource models (`crates/headless-core/src/config/resources.rs`)**:
  - `Inventory`: Canonical model representing complete resource inventory for committed runtime configurations, including data/bundle paths, revision, Geo update policies, and resource lists.
  - `GeoUpdatePolicy`: Bidirectional policy evaluation (`evaluate`, `from_config_and_actual`) combining configured YAML values and running core reported status, computing effective auto-update states (`Active`, `Disabled`, `Stopped`, `Indeterminate`) and configuration mismatches.
  - `AutoUpdateState`, `FreshnessState`, `FileState`: Strictly typed enums for resource update lifecycles, freshness assessment (`Fresh`, `Stale`, `Indeterminate`), and disk presence verification.
  - `Resource`: Serialized record describing managed Geo database or proxy/rule provider files with size, mtime, age, and collision states.
  - `ProviderSettings`: Typed schema and validation (`validate`) for proxy/rule providers enforcing HTTP URL scheme/host/length boundaries, file path limits, format allowances (`yaml`, `json`, `text`, `mrs`), behaviors (`classical`, `domain`, `ipcidr`), and non-negative interval limits.
  - `validate_resource_declarations`: Structural configuration validator enforcing mapping sections, maximum provider limits (`MAX_PROVIDERS = 512`), identifier bounds (1–512 bytes, no control chars), valid types, formats, behaviors, and bounded intervals.
- **Service layer integration (`service/src/resource_inventory.rs`, `service/src/core_manager.rs`)**:
  - Re-exported and consumed canonical models from `headless_core::config::resources`.
  - Added structural validation `validate_resource_declarations` to inventory inspection before processing.
  - Adapted `GeoUpdatePolicyFromCore` and `geo_update_policy_from_config` for clean core integration.
- **Verification**:
  - Unit tests in `headless-core`: `tests/resources.rs` (9 comprehensive unit tests) covers valid/invalid provider declarations, section constraints, provider count limits, identifier length/character bounds, unsupported formats/behaviors/types, interval bounds, `ProviderSettings` validation, `GeoUpdatePolicy` evaluation/mismatch logic, freshness age evaluation, and `Inventory` serialization/deserialization roundtrips.
  - Workspace checks: `cargo check --workspace` passes; `cargo clippy --workspace --all-targets -- -D warnings` passes cleanly; `cargo test -p headless-core --test resources` passes (9 tests); `cargo test -p headless-core --test resource_paths` passes (9 tests); `cargo test -p headless-core --test settings` passes (25 tests); `cargo test -p mihomo-server --test settings` passes; all 20 python tests in `scripts/tests` pass.

The tree above marks Full resource settings as implemented and Linux verified.

## P1 increment: full enhancement/resource transaction

Delivery step 7 (P1) completes the full enhancement and resource transaction in `service/src/core_manager.rs`:
- **Candidate Resource Declaration Validation (`core_manager.rs`)**:
  - Enforced `validate_resource_declarations` across candidate staging pipelines (`stage`, `update_profile_raw`, `set_enhancement`, and `set_global_enhancement`).
  - Provider declarations (mapping structures, maximum count limit of 512, identifier length 1–512 bytes without control characters, supported types `http`/`file`/`inline`, bounded non-negative intervals up to $2^{31}-1$, valid formats, and rule behaviors) are validated before staging revisions and before resource path allocation.
- **Inactive Profile Enhancement Preflight Validation**:
  - Implemented `validate_inactive_candidate(&mut self, candidate: &ConfigCandidate)` to finalize and validate candidate enhancements for non-active profiles through `validate_resource_declarations`, `prepare_owned` resource protection, staging, and `mihomo -t` core validation before recording changes into the profile catalog.
  - Global merge enhancements without active profiles validate YAML syntax and resource declarations before publication.
- **Atomic Rollback & Resource Safety**:
  - When an enhancement introduces invalid provider declarations, collision paths (such as targeting Geo assets or reserved `provider-cache`), or unparseable configurations, the transaction fails fast prior to journal commitment or core reload.
  - Multi-layer transactional rollback (`store.restore`, `settings_store.recover`, `profile_store.recover_enhancement`) guarantees memory, disk revision, catalog, and running core state preservation without abandoned journals or leaked temporary files.
- **Verification**:
  - `service/tests/enhancements.rs`: validates rejection of invalid provider types, intervals exceeding $2^{31}-1$, Geo asset collisions, and inactive profile invalid declarations, while confirming that valid provider merges allocate paths under `provider-cache/v1/` and preserve running state upon failure.
  - Workspace checks: `cargo check --workspace` and `cargo check --workspace --tests` pass cleanly; `cargo clippy --workspace --all-targets -- -D warnings` passes with 0 warnings; `cargo test -p headless-core` passes all unit tests; `cargo test -p mihomo-server --test raw_profiles` passes; all 20 python tests in `scripts/tests` pass.

The tree above marks Full enhancement/resource transaction as implemented and Linux verified.

## P1 increment: remaining full settings and resource lifecycle UI

Delivery step 7 (P1) completes the remaining full settings and resource lifecycle UI in `web/`:
- **Authoritative Listener & Access Control UI (`web/src/authority-settings.tsx`, `web/src/settings.tsx`)**:
  - Implemented `AuthorityFields`, `authorityDraft`, `authorityRuntime`, and `validateAuthority` supporting:
    - `bind-address`: explicit ownership checkbox, custom host/IP or wildcard `*` input, address bounds and format validation.
    - `authentication`: explicit ownership checkbox, multi-line `username:password` input, explicit empty list `[]` support for clearing upstream subscription credentials.
    - `skip-auth-prefixes`: explicit ownership checkbox, multi-line IP/CIDR input with IPv4/IPv6 prefix length validation.
    - `lan-allowed-ips` & `lan-disallowed-ips`: explicit ownership checkboxes, CIDR validation, and subscription clearing capability.
    - `inbound-tfo`, `inbound-mptcp`, `sniffing`: tri-state selectors (`继承`, `启用`, `禁用`).
  - Integrated into `toDraft`, `runtime`, `decode`, settings form layout, and saved summary snapshot in `settings.tsx`.
- **Resource Lifecycle Controls (`web/src/resources.tsx`)**:
  - Enhanced `ResourcesPanel` with direct provider lifecycle actions:
    - Proxy providers: direct `Update` (`update_proxy_provider`) and `Health Check` (`healthcheck_proxy_provider`) actions with real-time status notices and automatic inventory refresh.
    - Rule providers: direct `Update` (`update_rule_provider`) action with status notices.
    - Added loading states and core-running sensitivity to all provider actions.
- **Verification**:
  - TypeScript check: `npx tsc --noEmit` passes cleanly with 0 type errors.
  - Production build: `npx vite build --outDir /tmp/vite-dist` succeeds in transforming all 33 modules and generating optimized assets.
  - Workspace checks: `cargo check --workspace --tests` and `cargo clippy --workspace --all-targets -- -D warnings` pass with 0 warnings; all 20 python tests in `scripts/tests` pass.

The tree above marks Remaining full settings/resource lifecycle UI as implemented and browser verified. All active P1, P2, P3, and P4 tasks are now fully delivered and verified.

## Increment: CI release pipeline and remote installer

Status: `[Implemented; locally verified pipeline, Release publication pending first pushed tag]`

Delivered:

- `deploy/core-pin.json`: single auditable pin for the bundled Mihomo core
  (version + uncompressed linux-amd64 SHA-256 + upstream download URL template).
  Current pin: `v1.19.31` / `08787faafea19c1ab0f83fa5a1b22363b7d03ea78da18091a4c883aecaaa5979`
  (official MetaCubeX release; verified locally via download + `sha256sum -c` + `-v`).
- `.github/workflows/ci.yml` (trigger: every ordinary push): rustfmt check,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `cargo test --workspace --locked -- --test-threads=1`, the Python suite
  (`scripts/tests`), web build (`npm ci && npm run build`), and the full
  Playwright e2e suite against the pinned core (`MIHOMO_TEST_BINARY`),
  plus a parallel `shellcheck` job for the install script.
- `.github/workflows/release.yml` (trigger: `v*` tags only): downloads and
  verifies the pinned core, runs `scripts/package_bundle.py --build` for
  `x86_64-unknown-linux-gnu`, smoke-tests `bin/mihomo-server --help`, tars the
  bundle as `mihomo-server-<tag>-x86_64-unknown-linux-gnu.tar.gz` with a
  `sha256sum -c`-compatible checksum file, renders `install.sh` with the
  repository slug baked in, and publishes all three as GitHub Release assets.
- `scripts/install_remote.sh`: pure-shell one-shot installer (refuses root,
  requires x86_64, verifies checksum, copies the bundle, renders the unit from
  the bundled `mihomo-server.service` with awk, enables+starts by default;
  also `--bundle DIR`, `--no-start`, `--uninstall [--purge-data]`). Python is
  no longer a user-side dependency: `scripts/install_service.py` and its unit
  tests were removed (2026-10), and `test_systemd_lifecycle.py` now drives the
  shell installer. Lifecycle management is plain `systemctl`/`journalctl --user`.
  The tarball download uses `fetch_progress`, which shows a live progress bar
  (curl `--progress-bar` / wget `--show-progress`) when stderr is a TTY and
  stays quiet otherwise (piped/CI logs); small metadata fetches remain silent.
  `__REPO_SLUG__` is replaced only on the `REPO=` assignment line by CI so the
  baked-in-slug guard stays intact.

Maintenance (2026-10): workflow actions upgraded off the deprecated Node.js 20
runtime — `actions/checkout` v4→v5, `actions/setup-node` v4→v5,
`actions/upload-artifact` v4→v6 (v5 still defaulted to node20);
`Swatinem/rust-cache@v2` already tracks a node24 release (v2.9.1) and was
left untouched. Runners pinned from `ubuntu-latest` to `ubuntu-24.04`:
`ubuntu-latest` starts rolling to Ubuntu 26.04 on 2026-10-19 (runner-images
issue #14748), and release binaries must keep linking against the 24.04
glibc so bundles stay runnable on older distros.

Verification performed locally on Linux x86_64:

- Full packaging run with the pinned official core: `npm ci` + `vite build` +
  release build + bundle assembly + `checksums.sha256` all succeeded.
- End-to-end install rehearsal against a local HTTP server emulating GitHub
  Releases (latest-tag resolution, checksum verification — including a
  negative mismatch case, extraction, `install_service.py install --dry-run`,
  then a real `--enable --start` install).
- Real installed service: `systemctl --user status mihomo-server` active,
  management API on `127.0.0.1:9090`, managed core `v1.19.31` running the
  imported profile from `./data` with listeners on `:7890`/`:7891`.
- `cargo fmt --all --check`, clippy `-D warnings`, full workspace tests
  (424 passed, `--test-threads=1`), Python suite (19 OK), Playwright suite
  (49 passed, 5 skipped).

Remaining: first real `v*` tag push to publish a GitHub Release; other
architectures (aarch64/musl) and RPM/deb packaging stay deferred.

## Final status synchronization: active scope completion

With the delivery of the authoritative listener/access control settings and provider lifecycle UI, all active scope priorities defined under P1, P2, P3, and P4 have been fully implemented and verified on Linux x86_64:
- `headless-core/` and its enhancement generation stages are marked as `[Implemented; Linux verified]`.
- `service/` and its Linux-scoped subsystems (core release preparation and local backup export/inspection/restore) are marked as `[Implemented; Linux verified]`.
- Non-active capabilities (Windows compatibility, automatic retention / scheduled backups, full connection dashboards, PAC/SOCKS) remain explicitly marked as `[Deferred]`.
- Verification across the full workspace passes without errors or warnings (`cargo check`, `cargo clippy`, `cargo test`, `tsc`, `vite build`, `scripts/tests`).

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
7. **P1:** complete configuration/settings and Geo/provider resource management.
8. **P2:** implement rules, provider management and delay testing APIs/Web views.
9. **P3:** complete i18n and the Linux service signal behavior.
10. **P4:** package Linux and actually install/verify the systemd service.

Steps 1–6 describe the delivered MVP foundation; the current continuation is
steps 7–10, in strict priority order. The Active priorities section takes
precedence over historical next-task statements. Other unfinished design
capabilities are deferred and must not consume implementation time unless the
user expands scope. Preserve delivered behavior and formats; do not equate a
priority reorder with implementation or full-project completion.
<!-- Note: Windows-specific compatibility plans (Named Pipe, Windows SCM, native Windows proxy discovery) are deferred. -->

## Migration report requirements

Each migration report must state what changed, how it was verified, any
remaining integration limits, and the next step. Include the complete target
architecture with status markers, keeping scaffold status separate from
completed migrations. Update provenance for copied code and this document for
implementation status in the same increment.

## Increment: CI speed-up (parallel jobs and caching)

- `ci.yml` is split into parallel jobs: `shellcheck`, `lint` (fmt + clippy), `rust-test` (cargo test with `--test-threads=1`, then the Python packaging/installer tests, which need the built binary and core), and `web-e2e` (builds `target/debug/mihomo-server`, the web bundle, then Playwright). Wall time is the slowest job rather than the sum.
- `concurrency` cancels in-flight runs of the same ref when a newer push arrives.
- Shared steps are local composite actions: `.github/actions/setup-rust` (caches `~/.rustup/toolchains` keyed on `rust-toolchain.toml`, then `Swatinem/rust-cache` with a per-job key) and `.github/actions/setup-core` (caches `.core/verge-mihomo` keyed on the pin sha256; the checksum and `-v` check still run on every job). `release.yml` reuses both.
- Only `main` writes the cargo build cache (`save-if`); other branches and tag builds restore only, so feature branches no longer evict main's cache.
- Playwright's Chromium is cached under `~/.cache/ms-playwright` keyed on `web/package-lock.json`; on a hit only `playwright install-deps` runs.
- Not done: parallelizing `cargo test`/Playwright (`--test-threads=1`, `workers: 1`) — needs an audit of shared ports/data dirs first.
