# Browser UI

Minimal React views connect to the Rust service through authenticated HTTP commands
and first-frame-authenticated WebSockets. Production runs the Rust service and its
owned Mihomo child; no Node or Vite server is required.

## Build and run

From the repository root:

```sh
npm --prefix web ci
npm --prefix web run build
cargo build -p mihomo-server --locked
target/debug/mihomo-server --mihomo /usr/bin/verge-mihomo \
  --data-dir ./data --web-dir ./web/dist --config ./examples/minimal.yaml
```

Open `http://127.0.0.1:9090` and enter the private token from
`./data/management-token`. API and WebSockets use this same origin. Credentials
remain in React memory, so reload/logout requires login again. The `dev` script
previews Vite assets only; use the Rust-served build for authenticated workflows.
Rebuild assets after editing the frontend and refresh the browser.

## Current scope

- Centered responsive overview: core lifecycle/state, traffic, memory, connection
  count, actual Direct / Rule / Global mode and core-reported TUN status.
- Visible TUN switches on Overview and at the top of Settings: save/apply
  immediately, preserve advanced parameters and verify persisted/core state.
  Stopped cores stay stopped; unsaved settings drafts block immediate toggling.
- Compact settings rows with expandable advanced sections, question-mark help
  (hover/focus/touch), and collapsed core/resource/saved-state diagnostics.
- Shared top-center toast stack with individual close controls and eight-second
  expiry; successful operations remain visible when navigating between pages.
- Local YAML file/content import, saved-profile selection and current profile.
- Direct remote URL download/import with optional name and reported usage display.
  Import preserves the current profile; downloaded content is activated separately.
- Manual remote refresh preserves UID/title/node records and updates reported usage.
  Current-profile updates validate/apply with failure recovery; other profiles update
  only their saved content. A stopped core stays stopped.
- Profile title/description and supported remote URL/download-option editing. Changes
  save metadata without fetching/applying; explicit refresh uses the new settings.
- Noncurrent local/remote deletion with confirmation/cancellation and active-UID
  protection even while stopped. Cleanup uses the service recovery journal.
- Linked YAML merge editor and detach: preserve raw subscriptions, validate/apply
  active changes, use stored merges on selection/refresh and restore after restart.
  Auxiliary catalog rows are excluded from base-profile cards.
- Linked rules/proxies/groups sequence editor with explicit prepend/append/delete
  lists, type switching and detach. Current changes validate/apply; inactive ones
  apply when selected. Switching types discards unsaved editor text.
- Linked JavaScript editor/read/detach (Linux): synchronous main(config, name),
  bounded execution before save, visible failures and console output in Logs.
- Complete runtime YAML editing with validation and visible failures. `edit_config`
  preserves the current profile and node records; it does not rewrite raw profile files.
- Proxy-group selection and automatic-group unfix through the backend.
- Bounded core stdout/stderr view, event reconnect and per-view feed cancellation.

All writes use the backend lifecycle actor. HTTP/WS adapters are in `src/api.ts`,
transport types in `src/types.ts`, views in `src/main.tsx` and styles in
`src/style.css`. React 19.3, TypeScript 6.0.3 and Vite 8.3 are pinned in package-lock.
These are new minimal browser views, not a completed migration of upstream desktop pages.
Scheduled refresh/proxy modes, raw editing and cascade deletion, global/settings integration, rules/connections/provider pages,
advanced settings, upgrades and backups remain pending in
[the complete architecture](../docs/ARCHITECTURE.md).

## Real-browser validation

The Playwright tests start the built Rust service on an ephemeral loopback port
with private temporary data and serves `dist` directly. They exercise a real
Mihomo core, invalid startup/configuration, login, local file import, node choice,
config editing, remote URL import/refresh, invalid update preservation and saved catalog/node restoration,
metadata editing, protected local/remote deletion, saved file cleanup and persistence,
linked merge save/replace/reject/restart/detach with preserved raw content,
all three sequence types with restart/node restoration and mobile detach,
script source/logs, syntax/execution/core rejection, replace/restart and mobile detach,
reconnect, realtime metrics, mobile
layout and logout. Cleanup sends SIGTERM and requires successful service exit.

```sh
cargo build -p mihomo-server --locked
npm --prefix web run build
# Install Chromium for Playwright if not already available:
cd web
npx playwright install chromium
MIHOMO_TEST_BINARY=/usr/bin/verge-mihomo npm test
```

The test requires local socket/process permissions and a working Mihomo binary;
`MIHOMO_TEST_BINARY` defaults to `/usr/bin/verge-mihomo`. It creates no production
profile or credential. Generated screenshots/results and built assets are ignored.

To verify the same browser workflow through an already prepared release bundle,
run from `web/` with `MIHOMO_TEST_BUNDLE=/absolute/path/to/bundle npm test`.
The test launches the bundle's exec launcher and its persistent managed core,
serves the bundle's built assets and uses isolated temporary data. Prepare a
fresh bundle from the current source/assets before this check; see
[deployment instructions](../docs/DEPLOYMENT.md).
