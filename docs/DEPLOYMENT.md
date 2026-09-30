# Initial Linux deployment

Build tools require Python 3.11 or newer, the pinned Rust toolchain, and a Node/npm
version supported by the locked Vite release.

The verified deployment path is one foreground Rust service launched from a local
bundle. It serves the Web build and owns one independent Mihomo child. The first
bundle supports `x86_64-unknown-linux-gnu`; other targets and containers remain
pending. No Node/Vite, Python, download or second service is needed at runtime.

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
├── mihomo-server.service           # optional user systemd unit template
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

Choose an absolute persistent data directory outside the bundle:

```sh
MIHOMO_SERVER_DATA_DIR=/absolute/path/to/private-data /path/to/bundle/launch
```

Open `http://127.0.0.1:9090` and enter the token from
`/absolute/path/to/private-data/management-token`. The default proxy example uses
loopback port 7890 and does not enable TUN. Use `--config /absolute/path/bootstrap.yaml`
for different first-run settings, or `--no-start` to begin stopped. Listener/origin
arguments can be appended to launch; existing Host/Origin and token policies apply.
A failed bootstrap leaves the page available for local profile import and repair.

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

## Linux user systemd service management

`deploy/mihomo-server.service` runs the bundle launcher as a systemd user service,
defaulting to `%h/.local/opt/mihomo-server/launch` with persistent data in
`%h/.local/share/mihomo-server`.

Use `scripts/install_service.py` to automate bundle installation, unit registration,
and daemon lifecycle management:

```sh
# Install bundle to ~/.local/opt/mihomo-server and register systemd user unit:
python3 scripts/install_service.py install --bundle /path/to/bundle --enable --start

# Inspect rendered unit or run dry-run:
python3 scripts/install_service.py unit
python3 scripts/install_service.py install --bundle /path/to/bundle --dry-run

# Manage service lifecycle:
python3 scripts/install_service.py status
python3 scripts/install_service.py is-active
python3 scripts/install_service.py logs -n 50
python3 scripts/install_service.py restart
python3 scripts/install_service.py stop

# Uninstall unit without purging data:
python3 scripts/install_service.py uninstall
```

The unit uses `KillMode=mixed` so SIGTERM reaches Rust first and its child is
reaped through the shared shutdown path; the remaining control group is killed
only if shutdown exceeds the 30-second deadline. `UMask=0077` makes service-created data
private. Actual user systemd service lifecycle—including boot/start, authentication,
process supervision, child Mihomo reaping (`ESRCH`), journalctl logging, configuration
restoration, and real proxy traffic/node selection—is fully implemented and Linux-verified.

## Install from a GitHub Release

Releases are produced by `.github/workflows/release.yml` on `v*` tags. Ordinary
pushes run the full test suite via `.github/workflows/ci.yml` instead. Each
release publishes:

- `mihomo-server-<tag>-x86_64-linux-gnu.tar.gz` — the pinned bundle
- `mihomo-server-<tag>-x86_64-linux-gnu.tar.gz.sha256`
- `install.sh` — a rendered copy of `scripts/install_remote.sh` with the
  publishing repository slug baked in

The bundle embeds the pinned Mihomo core recorded in `deploy/core-pin.json`
(version + uncompressed-binary SHA-256, verified against the upstream release
before packaging). Upgrading the pinned core means editing that one file in a
PR.

### One-shot install

```sh
curl -fsSL https://github.com/OWNER/REPO/releases/latest/download/install.sh \
  | bash -s -- --enable --start
```

The script refuses root, requires `x86_64`, verifies the tarball checksum,
downloads `scripts/install_service.py` from the same tag and installs the
systemd user service. Extra arguments after `--` are passed to the installer
(e.g. `--listen 127.0.0.1:9090 --data-dir ~/.local/share/mihomo-server`);
with none it defaults to `--enable --start`.

### Manual install from a downloaded tarball

```sh
tar -xzf mihomo-server-<tag>-x86_64-linux-gnu.tar.gz
python3 scripts/install_service.py install --bundle mihomo-server-<tag>-x86_64-linux-gnu --enable --start
```

## Validation

```sh
cargo check --workspace --locked
cargo test --workspace --locked
python3 -m unittest discover -s scripts/tests -v
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo cargo test -p mihomo-server \
  --test deployment --locked -- --ignored --test-threads=1
```

The validation suite covers:
- Python test suite (`scripts/tests/test_package_bundle.py`, `scripts/tests/test_install_service.py`, and `scripts/tests/test_systemd_lifecycle.py`), verifying bundle layout, checksums, license inventory, unit generation, live systemd startup/restart/stop, child process reaping, and real proxy selection from `./data`.
- Opt-in Rust deployment test (`service/tests/deployment.rs`), packaging the actual service/core, launching from an unrelated working directory, testing first-use initialization, local-profile import/validation/start/node/config changes, failed validation, service restart, restored records and retained managed core, and requiring SIGTERM child reaping.
