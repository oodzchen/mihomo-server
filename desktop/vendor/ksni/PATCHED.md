# Vendored ksni 0.3.6

Only the library (`src/`, license, a trimmed manifest) of
[ksni 0.3.6](https://crates.io/crates/ksni/0.3.6), with one change:

- `org.kde.StatusNotifierItem.ProvideXdgActivationToken` is implemented. KDE
  Plasma calls it just before `Activate` with an XDG activation token; without
  it KWin refuses to raise the window the click opens and only marks it as
  demanding attention. The token is stored crate-wide and read with
  `ksni::take_xdg_activation_token()`, because tray-icon's `Tray` impl (which
  Tauri's tray uses) cannot forward it.

Upstream added the method as `Tray::provide_xdg_activation_token` in
iovxw/ksni@547cc42 (unreleased as of 0.3.6). Drop this copy and the
`[patch.crates-io]` entry once a ksni release has it and tray-icon exposes the
token.
