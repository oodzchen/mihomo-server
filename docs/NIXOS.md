# NixOS installation

The native NixOS installation owns the program and service definitions through
the system flake. Each selected user keeps independent XDG configuration, data,
management token, proxy ports and a mutable managed core. The default package
downloads the complete x86_64 Linux CI release; `switch` does not compile Rust or
the Web UI. A first installation may fetch ordinary Nixpkgs dependencies too.

## System configuration

Add the input to the system flake:

```nix
inputs.mihomo-server = {
  url = "github:oodzchen/mihomo-server";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Include `mihomo-server.nixosModules.default` in the host's `modules`, then add:

```nix
services.mihomo-server = {
  enable = true;
  users = [ "colin" ]; # Existing normal users; root is not supported.
  linger = true;      # Start at boot and keep running after logout (default).
  tun.enable = true;  # Authorize these users for the shared system TUN (default).
};
```

The optional desktop client has its own flake,
[mihomo-server-desktop](https://github.com/oodzchen/mihomo-server-desktop). Add it
as a second input that follows this one, include
`mihomo-server-desktop.nixosModules.default` and set
`programs.mihomo-server-desktop.enable = true;`:

```nix
inputs.mihomo-server-desktop = {
  url = "github:oodzchen/mihomo-server-desktop";
  inputs.nixpkgs.follows = "nixpkgs";
  inputs.mihomo-server.follows = "mihomo-server";
};
```

TUN DNS integration requires systemd-resolved (`services.resolved.enable = true`).
The module owns the `mihomo-tun` group, sticky slot registry, TUN lock, capability
wrapper and DNS polkit rules (enabling polkit). Do not run `setcap`, `usermod` or the installer for a
Nix-owned installation. TUN authorization grants system-wide network control;
the program's existing one-holder lock applies across users.

Apply the system configuration, then run commands as the selected user:

```sh
sudo nixos-rebuild switch --flake /etc/nixos#sfg3-nixos
mihomo-server info
mihomo-server status
```

The unit is `systemd.user.services.mihomo-server`, not a root system service.
Selected users start automatically. With `linger = false`, the user manager must
already be running or starts at the next login. With lingering, systemd starts
the user manager at boot. On a package/unit change, the module refreshes running
selected user managers and restarts their declared instances, including on a
system rollback. A stopped declared instance may therefore start at a switch.

`start`, `stop`, `restart`, `logs`, API commands and desktop discovery continue to
work. Autostart belongs to `services.mihomo-server.users`; CLI `enable`/`disable`
and the Web autostart switch cannot change it. Configuration and data defaults:

- `~/.config/mihomo-server/env`
- `~/.local/share/mihomo-server`

Custom paths use the same XDG rules as the script installation. In a user `env`
file, set `MIHOMO_SERVER_DATA_DIR` to an absolute path to preserve custom data,
or set `MIHOMO_SERVER_LISTEN`/`MIHOMO_SERVER_PUBLIC_ORIGIN` for remote access.
Restart the instance after editing. The old module options `user`, `dataDir`
and `listen` produce explicit migration errors rather than silently selecting
different state directories or a different service model.

## Updates and removal

```sh
# Update the program's release pin and NixOS integration.
sudo nix flake update mihomo-server --flake /etc/nixos
sudo nixos-rebuild switch --flake /etc/nixos#sfg3-nixos

# To update all system flake inputs instead:
sudo nix flake update --flake /etc/nixos
sudo nixos-rebuild switch --flake /etc/nixos#sfg3-nixos
```

Updating only the system `nixpkgs` input does not advance the `mihomo-server`
source/release pin. Main receives version/hash updates after CI publishes each
release; a development commit can still point to the most recent published
binary. To build that commit's code, explicitly select
`mihomo-server.packages.${pkgs.stdenv.hostPlatform.system}.server-source` as the
service `package`. That opt-in package compiles locked Rust and frontend sources.
Its reported version includes a development revision. Both server packages
currently support x86_64 Linux only; other architectures require new pinned
core/release assets.

Nix metadata follows the executable, not `/etc/NIXOS`. `mihomo-server update`
and `uninstall` refuse the remote installer for Nix packages, including binaries
predating this integration. `/opt` installations on NixOS without the module
retain the normal installer workflow; once the module provides the user unit, the
installer refuses to install or upgrade (only `--uninstall` still runs), because
its per-user unit would shadow the module's. The newer service API/Web explains Nix ownership and refuses
program upgrades and autostart changes too; those API/Web changes become available
in the first CI release containing them. The current precompiled pin is v0.2.13.
The desktop flake's packages select the Nix helper and block installation through
the script.

Releases up to v0.2.13 also have a listener readiness race: its core API may
answer before proxy listeners finish binding, causing a startup error reporting
port `0`. The current source waits for listeners, and keeps reporting persistent
port conflicts. This fix is included in the next CI release along with the Nix
API/Web changes; use that release for the complete native installation.

Remove a user from `services.mihomo-server.users` to stop their declared
instance and remove autostart, or set `services.mihomo-server.enable = false` to remove the module's
service and program. Stop running instances before removing the module; apply
the configuration with `switch`. User data is retained. Remove data explicitly
after stopping the instance and changing the configuration.

Runtime `core update` and Geo updates remain independent per-user operations.
System rollback restores the packaged program and resource seed, not mutable
user state or a separately upgraded core. Startup keeps newer/Alpha cores and
only promotes an older stable core to the package's bundled stable version.

## Migrating a script installation

Migration is explicit so old units cannot shadow the Nix unit and existing state
is preserved. Before switching, inspect `mihomo-server info` and record any
custom data/config paths. Stop the old user service:

```sh
systemctl --user disable --now mihomo-server.service
```

Move the installer-created user unit and drop-in directory outside the systemd
search path, keeping a backup. For default XDG paths, the files are:

```text
~/.config/systemd/user/mihomo-server.service
~/.config/systemd/user/mihomo-server.service.d/
```

Inspect custom drop-ins before moving them: copy any custom paths/settings into
the user `env` file or system configuration first. A custom `XDG_CONFIG_HOME` may
place these files elsewhere. Also remove only command links pointing to the old
`/opt/mihomo-server` installation, for example `~/.local/bin/mihomo-server` or
`/usr/local/bin/mihomo-server`. Otherwise they may precede the Nix command in PATH.
For graphical shell completions/manuals, prefer the Nix package's `share` files.

Switch the system, then `systemctl --user daemon-reload` and verify. A unit left
behind is reported by the switch itself (`mihomo-server-user-refresh.service`):
`warning: USER's mihomo-server.service is shadowed by PATH`.



```sh
readlink -f "$(command -v mihomo-server)"
systemctl --user show mihomo-server.service -p FragmentPath -p ExecStart
mihomo-server info
```

The executable and ExecStart should reference `/nix/store`. Retain existing user
data and `/var/lib/mihomo-server/slots` claims to preserve tokens and port slots.
Keep `/opt` until all users have migrated and verification succeeds. An old shared
update unit should then be disabled/removed; never run its installer against a
native Nix installation. Do not use installer `--purge` during migration.

## Verification

```sh
nix build .#server-bin .#checks.x86_64-linux.module
nix build .#checks.x86_64-linux.vm   # NixOS VM test; needs KVM
MIHOMO_TEST_NIX_PACKAGE="$(nix build .#server-bin --no-link --print-out-paths)" \
  python3 -m unittest scripts.tests.test_nix_runtime -v
```

The runtime test uses isolated temporary data, disables TUN and checks the embedded
version, upgrade protection, initialized core, Web UI, an actual local HTTP proxy
request, and token persistence across restart. It does not change the live system
configuration. Installation/launcher tests cover the existing script workflow.
The VM test boots the module with declared users and checks their store-based
instances and TUN capability, the per-user DNS polkit rule (a member cannot set
another member's `ms<uid>` link), Nix-owned update/uninstall, the shadowing
warning, and a switch adding a lingering user whose manager predates the
`mihomo-tun` group (the launcher's setuid `sg` path).
If a pre-Nix pin (v0.2.12, v0.2.13) encounters its known readiness race, only that runtime test
is skipped; version/ownership checks still run. Later release pins must pass it.
CI separately exercises the current source's readiness fix and port-conflict
rollback with a real core.
