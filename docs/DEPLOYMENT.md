# Initial Linux deployment

Build tools require Python 3.11 or newer, the pinned Rust toolchain, and a Node/npm
version supported by the locked Vite release.

The verified deployment path is one foreground Rust service launched from a local
bundle. It serves the Web build and owns one independent Mihomo child. The first
bundle supports `x86_64-unknown-linux-gnu`; other targets and containers remain
pending. No Node/Vite, Python, download or second service is needed at runtime.

Native NixOS installation and migration are documented in [NIXOS.md](NIXOS.md).
The Nix default consumes CI binaries; local source compilation is opt-in.

## Prepare a pinned bundle

Use an existing core obtained from the upstream release or pinned build resources.
Supply its expected **uncompressed binary** SHA-256 from your trusted build record;
the packager verifies it before executing `-v`, checks the pinned version and ELF
architecture, and records target/version/hash in the resource manifest. It never
queries latest releases or downloads during builds/startup. The initial package
contains one stable bootstrap core; Alpha bundle seeds and automated build-time
fetching remain pending. Once running, the authenticated `/core` page can upgrade
or repair the persistent core through stable or Alpha channels. Both support
force/no-op, durable rollback and installation readback; see the source
repository's `docs/RUNNING.md`.

Authenticated `POST /api/backup` exports a service snapshot. Upload its raw ZIP to
`POST /api/backup/inspect` with `Content-Type: application/zip` to check structure,
integrity and configuration references without applying it. Both share one
bounded operation slot. Export itself retains no archive and ZIPs are never
blindly extracted. Explicit Linux local storage uses `POST/GET /api/backups` and
`GET/DELETE /api/backups/{id}` with empty bodies and opaque IDs. A private
`<data-dir>/backups/` store permits 32 archives/256 MiB, without automatic pruning.
Downloads are verified before streaming and can use the normal restore upload
contract. Atomic create/delete receipts report durability_pending after fsync
acknowledgement failures. Startup removes only safe abandoned partial files.
Automatic retention/schedules, WebDAV and backup UI remain pending. Full request limits and
error codes are documented in the source repository's `docs/RUNNING.md`.
The same binary upload sent to `/api/backup/validate` additionally rehearses the
archived runtime and active profile regeneration using isolated Mihomo/script
probes, removes its disposable files and returns digests without applying data.
Restore a settled running or stopped core via `POST /api/backup/restore` with the same binary
body and one `X-Backup-Runtime: archived` or `regenerated` header. Archived policy
preserves the runtime snapshot's exact bytes; regenerated policy replaces manual
runtime edits from the active archived profile and settings. Protected provider
DNS requires regeneration and fresh confirmation before page overrides can be
enabled. Bootstrap-only archives require archived policy. A successful receipt
reports committed=true, applied revision/digest, cleanup_pending, core_running and
core_restarted; inspect status before retrying an interrupted request. A running
core reloads with verified ports or restarts as fallback; precommit failure restores
the old data/core/node records. A stopped core stays stopped. Unsettled lifecycle
states return 409; stop before retrying. See `docs/RUNNING.md` for the
full transaction/error contract. Restore scratch candidates are private under
`<data-dir>/restore-candidates/`; after exclusive data locking, startup reclaims
verified unleased orphans without following links. The namespace is service-owned.
Unsafe roots or cleanup I/O/budget failures stop startup for repair/retry. Legacy
unmarked `/tmp/ms-restore-*` directories require manual inspection.

```sh
python3 scripts/package_bundle.py --build \
  --target x86_64-unknown-linux-gnu \
  --mihomo /path/to/verge-mihomo --core-version v1.19.31 \
  --core-sha256 EXPECTED_64_HEX_SHA256 \
  --output /path/to/releases/mihomo-server-0.1.0
```

`--build` runs `npm ci`, the frontend build, and locked Cargo release compilation
for the explicit target. Initial dependency downloads require network access;
subsequent builds can use the dependency cache. To package already-built artifacts,
omit `--build` and specify `--service /path/to/mihomo-server --web-dir /path/to/web/dist`.
The output must be a new directory; packaging never installs over a running bundle
or writes runtime data. Web resources reject symlinks and special files.

```text
bundle/
├── launch                         # execs the sole foreground service
├── bin/mihomo-server
├── resources/
│   ├── manifest.json              # schema, exact target, core version/SHA-256, licenses
│   ├── core/verge-mihomo           # immutable initial independent core
│   ├── minimal.yaml               # bootstrap only; committed config wins
│   └── web/                       # built React assets
├── mihomo-server.service           # shared systemd user unit
├── mihomo-server-user              # per-user control of the shared installation
├── install.sh                     # installer copy for offline `mihomo-server uninstall`
├── share/                         # mihomo-server(1), bash and zsh completions
├── checksums.sha256
├── LICENSE
├── LICENSES.txt                   # third-party license and dependency inventory
└── docs/{DEPLOYMENT,UPSTREAM}.md
```

Check the prepared bundle from its root with `sha256sum -c checksums.sha256`.
The manifest is a local build record, not a cryptographic publisher signature.
See LICENSES.txt and UPSTREAM.md for source revisions, third-party dependency
licenses, and the client's unresolved upstream license declaration before external
redistribution; this path prepares local artifacts.

## Foreground launch and stop

The launcher defaults to `${XDG_DATA_HOME:-$HOME/.local/share}/mihomo-server`
(ignoring relative XDG values). To use an explicit persistent directory outside
the bundle:

```sh
MIHOMO_SERVER_DATA_DIR=/absolute/path/to/private-data /path/to/bundle/launch
```

Open `http://127.0.0.1:9090` and enter the token from
`/absolute/path/to/private-data/management-token`. The default proxy example uses
loopback port 7890 and does not enable TUN. Use `--config /absolute/path/bootstrap.yaml`
for different first-run settings, or `--no-start` to begin stopped. Listener/origin
arguments can be appended to launch; existing Host/Origin and token policies apply.
A failed bootstrap leaves the page available for local profile import and repair.

`launch` runs `bin/mihomo-server` with service options; `mihomo-server serve
[OPTIONS]` is the explicit form, and an invocation whose first argument is a service
option (as `launch` and older units use) still runs the service. Any other first
argument is a command of the `mihomo-server` control interface (below).
`launch` uses `exec`, so its PID is the Rust service PID. Ctrl-C or SIGTERM to that
PID stops command admission, closes HTTP/WS, cancels operations and terminates/reaps
the owned core before exiting. Run it in the foreground or under one process manager.
The Rust service must have up to 30 seconds for shutdown. The launcher is not a
background daemon and does not register a separate Mihomo service.

## Resource and persistence boundary

`--resource-dir` identifies the read-only bundle resources. It conflicts with
`--mihomo`, which remains the API-only/manual external-binary mode. Resource mode
resolves Web assets and bootstrap from that directory; explicit `--web-dir` or
`--config` can override those two paths. The manifest target must match the Rust
binary's build target. Invalid resource initialization stops management startup.

The manager acquires the data lock before initializing the core. On first use,
it verifies the bundled core bytes against the manifest, copies them to a private
staging file, syncs it and atomically publishes the executable without overwriting
an existing file. The managed binary defaults to `<data-dir>/core/verge-mihomo`.
`--core-dir /absolute/persistent/core` selects another private directory. That
location must be writable by the service for future core upgrade staging. Existing
nonempty regular executable cores are retained, even if their hash differs from
the bundle pin; this preserves independently upgraded cores. Existing symlinks,
nonexecutables or unsafe directory/file permissions are rejected. Automatic core
upgrade and backup schedules remain pending; manual core upgrades already support
durable rollback; configuration-backup restoration is available while running or stopped.

Keep data outside versioned releases. It contains runtime revisions/journal,
profiles and node records, credentials, the owned core, and runtime socket files.
For a service update, stop the current service, prepare a fresh bundle, launch it
with the same absolute data/core directories, and keep the previous bundle for
service rollback. The committed runtime takes precedence over the new bootstrap;
existing credentials and managed core are not replaced. Never delete persistent
data as part of bundle replacement. Optional pinned Geo seeding is available as
described below; full resource update/rollback and provider cache ownership remain
pending.

## Include existing Geo files for first-use initialization

Provide a directory of existing files and a JSON integrity manifest alongside the
normal bundle arguments:

```sh
python3 scripts/package_bundle.py \
  --target x86_64-unknown-linux-gnu \
  --mihomo /path/to/verge-mihomo --core-version v1.19.31 \
  --core-sha256 EXPECTED_CORE_SHA256 --output /path/to/fresh-bundle \
  --geo-dir /path/to/geo-files --geo-manifest /path/to/geo-pins.json
```

The Geo pin file maps supported filenames to exact sizes and expected SHA-256:

```json
{
  "geoip.metadb": {
    "bytes": 123456,
    "sha256": "EXPECTED_64_HEX_DIGIT_SHA256"
  }
}
```

Replace the example size/digest with trusted pins for the input. Supported names
are `Country.mmdb`, `ASN.mmdb`, `geoip.dat`, `geosite.dat`, `geoip.metadb` and
`GeoSite.dat`. Files must be nonempty, at most 128 MiB each and 256 MiB total.
Both optional arguments are required together. The packager reads only named seed
files, checks pins before/after copying into `resources/geo`, records them in the
resource manifest's optional `geo` map and includes them in `checksums.sha256`.
Old manifests without this map remain supported. No Geo download is performed.

At startup, while holding the data lock, only missing declared Geo files are
initialized. Existing declared nonempty regular data files remain authoritative even if
the bundle changes; links, empty or oversized existing entries stop initialization.
Seeds are staged privately and checked before publication as independent 0600
files. Known partial staging is recovered after interruption. Publication is
atomic per file, so a partially published verified set resumes at next startup.
Inspect installed files in the Web settings resource panel or the authenticated
`resources` command. Geo updates and format-specific validation are not provided
by this initialization path: matching SHA-256 proves integrity, not validity of
every Geo database format. Select suitable assets for the configuration's Geo mode.

## Shared Linux installation

Releases are produced by `.github/workflows/release.yml` on `v*` tags; the bundle builds alongside CI and is published once CI passes.
They contain a pinned tarball, its SHA-256 checksum and a rendered `install.sh`.
The embedded core pin is recorded in `deploy/core-pin.json`.

As a normal user, run without arguments:

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash
```

The installer verifies the download before sudo elevation, deploys the shared
bundle, adds the caller to `mihomo-tun`, enables linger and starts the user's
instance. Readiness requires an authenticated API and running core; TUN
capability and linger are also checked. Group refresh uses `sg` for the service
process, without restarting the user manager. Direct root invocation deploys
shared files only; `SUDO_USER` identifies a caller when invoked through sudo.

Required tools: Bash, systemd (`systemctl`, `loginctl`, `systemd-run`), tar,
SHA-256 (`sha256sum` or `shasum`), curl or wget, getent, groupadd, usermod,
runuser, sg and libcap's setcap/getcap; non-root invocation also requires sudo.
Runtime does not require Python or Node.

Graphical callers without a terminal (the desktop client) set
`MIHOMO_INSTALL_ELEVATE=pkexec`: the same root step then runs through `pkexec`,
and polkit's own dialog asks for authorization. The default remains sudo.

Layout:

- `/opt/mihomo-server/releases/<tag>` and atomic `current` link: root-owned,
  current and previous releases retained.
- `/etc/systemd/user/mihomo-server.service`: shared user unit, copied verbatim.
- `/usr/local/bin/mihomo-server`: the control command (symlink into `current`),
  with `/usr/local/share/man/man1/mihomo-server.1` and bash/zsh completions under
  `/usr/local/share/{bash-completion/completions,zsh/site-functions}`. An existing
  non-symlink file at any of these paths is kept and reported, never replaced.
- `/usr/local/bin/mihomo-server-user`: user helper (service lifecycle and paths).
- `/var/lib/mihomo-server/slots`: root-owned 1777 stable slot registry.
- `${XDG_CONFIG_HOME:-$HOME/.config}/mihomo-server/env`: optional startup settings.
- `${XDG_DATA_HOME:-$HOME/.local/share}/mihomo-server`: existing persistent layout.
  Runtime sockets remain in `<data-dir>/run`; no state/cache/backup format migration.

The helper persists resolved XDG config/data paths in a private managed user
drop-in, found using the user manager's configuration root. `%E` uses the manager
configuration root; the drop-in supplies the actual EnvironmentFile path when
terminal XDG values differ. Unset variables reuse saved values; empty/relative
values use XDG defaults. Explicit `MIHOMO_SERVER_DATA_DIR` remains supported.

For remote access, edit the displayed `env` file:

```ini
MIHOMO_SERVER_LISTEN=0.0.0.0:9090
MIHOMO_SERVER_PUBLIC_ORIGIN=https://proxy.example.com
```

Named settings override matching options in legacy `MIHOMO_SERVER_ARGS`.
Restart the instance to apply changes. API Host/origin and token checks remain
active, including for installer readiness checks behind a public origin.

### The `mihomo-server` command

`mihomo-server COMMAND` controls the invoking user's instance (see
`mihomo-server --help` and `man mihomo-server`):

```sh
mihomo-server status                    # service, core, subscription, mode, TUN, ports
mihomo-server start | stop | restart    # restart/start wait for a running core and API
mihomo-server enable | disable          # also at boot / not at boot; data is kept
mihomo-server info | token | logs [-n 100]
mihomo-server sub [list|use|update|add|remove]
mihomo-server proxy [list [GROUP]|select [GROUP] NODE|test [TARGET]|unfix GROUP]
mihomo-server mode [rule|global|direct]
mihomo-server tun [on|off]
mihomo-server core [version|update [--alpha]]
mihomo-server update                    # latest release installer, unless up to date
mihomo-server uninstall [--purge]       # bundled installer; asks unless --yes
```

Service lifecycle commands execute the release's `mihomo-server-user` helper.
API commands find the instance from the user's systemd unit: its main PID's
command line gives the `--listen`, `--public-origin` and `--data-dir` the launcher
passed (or the slot port from the registry), so wildcard listeners are reached on
loopback with the authorized public-origin Host, and the token is read from the
data directory. `--api URL` and `--token-file FILE` (or `MIHOMO_SERVER_API`,
`MIHOMO_SERVER_TOKEN_FILE`) address a foreground service instead; start, stop and
restart then control its core. Subscriptions, groups and nodes may be named
exactly, by list number, case-insensitively or by a unique part of the name; an
omitted operand is asked for on a terminal. `--json` prints machine-readable
results. Exit status is 0 on success, 1 on failure, 2 on invalid usage, and 3
from `status` when the instance is not running. Root has no instance.

`update` compares the installed release (the `releases/<tag>` directory it runs
from) with the tag GitHub's `releases/latest` redirects to, then runs the latest
published `install.sh`; `--force` reinstalls. The repository is fixed at build
time (`MIHOMO_SERVER_REPOSITORY`, set by the release workflow). Both `update` and
`uninstall` honor `MIHOMO_SERVER_INSTALLER` (a path or URL), used by the
multi-user test.

The helper remains available directly:

```sh
mihomo-server-user enable        # init is a compatibility alias
mihomo-server-user start         # start and verify without enabling at boot
mihomo-server-user stop
mihomo-server-user info          # also the default without arguments
mihomo-server-user token
mihomo-server-user status
mihomo-server-user logs [OPTS]   # follows; options replace -f for journalctl
mihomo-server-user restart
mihomo-server-user disable       # keep data
mihomo-server-user purge --yes   # delete own data/config and release slot
```

Other users opt in with `enable`; administrators separately grant their TUN and
linger with `usermod -aG mihomo-tun USER` and `loginctl enable-linger USER`.
Slot 0 uses management 9090, mixed proxy 7890 and DNS 1053. Slots 1–63 use
management `20000+10N`, proxy `20001+10N` and DNS `20002+10N`. Port conflicts are
reported rather than silently allocating alternatives. Existing slot-0 defaults
change on upgrade; explicit user ports are preserved.

Each instance runs its own managed core in `<data-dir>/core`, seeded from the
bundle and upgradable from the Web (stable/Alpha, with durable rollback) exactly
as in single-user mode. TUN members start that core through
`bin/mihomo-tun-exec`, a root:mihomo-tun 0750 copy of the service binary with
`cap_net_admin,cap_net_bind_service,cap_net_raw+ep`; it raises those as ambient
capabilities and executes the member's core. Re-running the installer upgrades an
older stable managed core to the bundle pin at the next start; newer and Alpha
cores are kept. Multi-user isolation still replaces subscription
listeners, retains explicit settings-page values, scopes TUN to `ms<uid>` and
`include-uid: [uid]`, assigns independent routing and fake-IP blocks and disables
host-wide auto-redirect. Shared-resolver traffic uses the existing domain sniffer.
TUN here is **system-wide**, like a single-user client: it has no
`include-uid`, carries every account's traffic, and Mihomo's `resolvectl` calls
point systemd-resolved at it, so every account's lookups get fake IPs that are
routed through it. A host has one such TUN at a time: the first TUN-group member
to enable TUN holds an `flock` on `/var/lib/mihomo-server/tun.lock`
(root:mihomo-tun 0640) until they turn it off or their service stops (the kernel
releases it, so a crashed holder never blocks the host). Other members see the
holder in the Web UI with their TUN switch disabled; their traffic follows the
holder's rules and their own proxy ports keep working. Nobody else can turn the
holder's TUN off; root can, e.g. `systemctl --user -M alice@ stop mihomo-server`.
An instance that starts with TUN saved while someone else holds it turns its TUN
setting off and starts without it. The installer adds
`/etc/polkit-1/rules.d/50-mihomo-server-tun.rules`, which allows those resolve1
link actions for each member's own `ms<uid>` link without a prompt and refuses
other members' links (a local-authority `.pkla` group grant on polkit 0.105).
Group members can give any program these network capabilities through the
launcher; authorize only trusted users.

Members can also upgrade from the Web UI's Service page. The installer adds the
root oneshot unit `/etc/systemd/system/mihomo-server-update.service` (runs
`mihomo-server update`, output in `/var/lib/mihomo-server/update.log`) and
`/etc/polkit-1/rules.d/50-mihomo-server-update.rules`, which lets `mihomo-tun`
members start only that unit without a password. It installs the latest
official release for everyone and restarts every running instance; it downloads
directly as root, not through anyone's proxy. polkit 0.105 cannot limit a grant
to one unit, so there the Web upgrade is refused; use `mihomo-server update`.

Re-running the installer upgrades the caller and other running instances. Other
stopped instances stay stopped. Known legacy user units are backed up, original
data and arguments are preserved, and the old program directory is retained.
Activation failure restores the old unit; custom units/drop-ins require review.

```sh
curl -fsSL https://github.com/oodzchen/mihomo-server/releases/latest/download/install.sh | bash -s -- --uninstall [--purge]
```

Uninstall stops every instance and removes the program, unit, helper, the
`/usr/local` command/manual/completion links pointing into `/opt/mihomo-server`, enablement,
slot registry, `mihomo-tun` group, the polkit TUN DNS rule, the update unit and
its polkit rule, and the lingering the installer enabled. User
data, env files and the helper's path drop-in are kept for a reinstall. `--purge`
also deletes, as each owner, every instance's data and configuration (resolved
from the saved XDG paths and env file), legacy unit backups and a migrated
`~/.local/opt/mihomo-server`. Public options are only `--help`, `--uninstall`
and `--purge`.
For manual local deployment, verify the downloaded tarball and use the foreground
launcher described above; local-bundle installer overrides are internal to tests.

## Desktop client

The desktop client is developed and released in its own repository,
[mihomo-server-desktop](https://github.com/oodzchen/mihomo-server-desktop), which
documents its build dependencies and packages. It pins `crates/management-client`
to a service release tag. `MIHOMO_SERVER_API` and `MIHOMO_SERVER_TOKEN_FILE`
point a development build at a foreground service, as for the command line.

## Validation

```sh
cargo check --workspace --locked
cargo test --workspace --locked
python3 -m unittest discover -s scripts/tests -v
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo cargo test -p mihomo-server \
  --test deployment --locked -- --ignored --test-threads=1
```

The validation suite covers:
- `service/tests/cli.rs` checks the command's usage, exit statuses and service routing; its opt-in live test drives a foreground service (subscriptions, node selection, modes, core start/stop) through `--api`. The multi-user container drives the installed command as real users: discovery behind a public-origin wildcard listener, status, subscriptions, node selection and delay tests with real nodes, mode, TUN switching and handover, stop/start, `update` and `uninstall`.
- Python tests verify packaging, integrity/transport failures, launcher argument boundaries and isolated service lifecycle. The privileged multi-user container tests the actual zero-argument download and sudo path, opt-in users, XDG persistence, immediate authorization, real-node traffic, Web core upgrade with TUN, installer core-pin upgrade, upgrade, container reboot, uninstall, reinstall and purge. The host lifecycle test uses a unique unit instead of installing shared host files.
- Opt-in Rust deployment test (`service/tests/deployment.rs`), packaging the actual service/core, launching from an unrelated working directory, testing first-use initialization, local-profile import/validation/start/node/config changes, failed validation, service restart, restored records and retained managed core, and requiring SIGTERM child reaping.
