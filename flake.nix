{
  description = "Headless Mihomo management service and web client";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        desktopRelease = {
          version = "0.2.13";
          hash = "sha256-gQIMqNRBTC36B2vXISfuDFQM4g8HpDf0JxsQPiLKPWY=";
        };

        desktop-source = pkgs.rustPlatform.buildRustPackage {
          pname = "mihomo-server-desktop";
          version = desktopRelease.version;
          src = ./.;
          MIHOMO_DESKTOP_VERSION = desktopRelease.version;

          cargoLock = {
            lockFile = ./desktop/Cargo.lock;
          };

          postUnpack = ''
            cp -f $sourceRoot/desktop/Cargo.lock $sourceRoot/Cargo.lock
          '';

          buildAndTestSubdir = "desktop";

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook3
          ];

          buildInputs = with pkgs; [
            gtk3
            webkitgtk_4_1
            glib
            glib-networking
            openssl
            libayatana-appindicator
            librsvg
            xdotool
          ];

          postInstall = ''
            install -Dm644 desktop/mihomo-server-desktop.desktop $out/share/applications/mihomo-server-desktop.desktop
            substituteInPlace $out/share/applications/mihomo-server-desktop.desktop \
              --replace-fail '{{exec}}' 'mihomo-server-desktop' \
              --replace-fail '{{icon}}' 'mihomo-server-desktop'

            install -Dm644 desktop/icons/src/app.svg $out/share/icons/hicolor/scalable/apps/mihomo-server-desktop.svg
            install -Dm644 desktop/icons/icon.png $out/share/icons/hicolor/512x512/apps/mihomo-server-desktop.png
            install -Dm644 desktop/icons/128x128@2x.png $out/share/icons/hicolor/256x256/apps/mihomo-server-desktop.png
            install -Dm644 desktop/icons/128x128.png $out/share/icons/hicolor/128x128/apps/mihomo-server-desktop.png
            install -Dm644 desktop/icons/32x32.png $out/share/icons/hicolor/32x32/apps/mihomo-server-desktop.png
            install -Dm644 desktop/icons/icon.png $out/share/pixmaps/mihomo-server-desktop.png
          '';
        };

        desktop-bin = pkgs.stdenv.mkDerivation rec {
          pname = "mihomo-server-desktop";
          version = desktopRelease.version;

          src = pkgs.fetchurl {
            url = "https://github.com/oodzchen/mihomo-server/releases/download/v${version}/mihomo-server-desktop-v${version}-x86_64.tar.gz";
            hash = desktopRelease.hash;
          };

          nativeBuildInputs = with pkgs; [
            autoPatchelfHook
            wrapGAppsHook3
          ];

          buildInputs = with pkgs; [
            gtk3
            webkitgtk_4_1
            glib
            glib-networking
            openssl
            libayatana-appindicator
            librsvg
            xdotool
          ];

          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r bin share $out/
            runHook postInstall
          '';
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "mihomo-server";
          version = "0.1.0";
          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          buildAndTestSubdir = "service";

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ ];

          postInstall = ''
            # Provide symlink for tun_exec capability launcher target
            ln -s $out/bin/mihomo-server $out/bin/mihomo-tun-exec
          '';
        };

        packages.desktop = if system == "x86_64-linux" then desktop-bin else desktop-source;
        packages.desktop-bin = desktop-bin;
        packages.desktop-source = desktop-source;

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustup
            cargo
            rustc
            nodejs
            python3
            pkg-config
            gtk3
            webkitgtk_4_1
            glib
            glib-networking
            openssl
            libayatana-appindicator
            librsvg
            xdotool
          ];
        };
      }
    ) // {
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.mihomo-server;
          desktopCfg = config.programs.mihomo-server-desktop;
          package = cfg.package;
        in {
          options.services.mihomo-server = {
            enable = lib.mkEnableOption "Headless Mihomo management service";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.default;
              description = "The mihomo-server package to use.";
            };

            user = lib.mkOption {
              type = lib.types.str;
              default = "root";
              description = "User to run the mihomo-server service under.";
            };

            dataDir = lib.mkOption {
              type = lib.types.str;
              default = "/var/lib/mihomo-server";
              description = "Path to the service data directory.";
            };

            listen = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1:9090";
              description = "Management address to listen on.";
            };

            tun = {
              enable = lib.mkOption {
                type = lib.types.bool;
                default = true;
                description = "Enable TUN mode permissions and polkit rules.";
              };
            };
          };

          options.programs.mihomo-server-desktop = {
            enable = lib.mkEnableOption "Mihomo Server Desktop client";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.desktop;
              description = "The mihomo-server-desktop package to use.";
            };
          };

          config = lib.mkMerge [
            (lib.mkIf cfg.enable {
              users.groups.mihomo-tun = {};
              users.users = lib.mkIf (cfg.user != "root") {
                ${cfg.user}.extraGroups = lib.mkIf cfg.tun.enable [ "mihomo-tun" ];
              };

              security.wrappers.mihomo-tun-exec = lib.mkIf cfg.tun.enable {
                source = "${package}/bin/mihomo-tun-exec";
                capabilities = "cap_net_admin,cap_net_bind_service,cap_net_raw+ep";
                owner = "root";
                group = "mihomo-tun";
                permissions = "0750";
              };

            security.polkit.extraConfig = lib.mkIf cfg.tun.enable ''
              polkit.addRule(function (action, subject) {
                var actions = [
                  "org.freedesktop.resolve1.set-dns-servers",
                  "org.freedesktop.resolve1.set-domains",
                  "org.freedesktop.resolve1.set-default-route",
                  "org.freedesktop.resolve1.revert"
                ];
                if (actions.indexOf(action.id) >= 0 && subject.isInGroup("mihomo-tun")) {
                  var link = action.lookup("interface");
                  if (!link) return polkit.Result.YES;
                  if (!/^ms[0-9]+$/.test(link)) return polkit.Result.NOT_HANDLED;
                  return polkit.Result.YES;
                }
              });
            '';

            systemd.services.mihomo-server = {
              description = "Headless Mihomo management service";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];

              serviceConfig = {
                Type = "simple";
                User = cfg.user;
                Group = if cfg.tun.enable then "mihomo-tun" else null;
                ExecStart = "${package}/bin/mihomo-server serve --data-dir ${cfg.dataDir} --listen ${cfg.listen}";
                Restart = "on-failure";
                RestartSec = 3;
                KillSignal = "SIGTERM";
                KillMode = "mixed";
                TimeoutStopSec = 30;
                UMask = "0077";
                AmbientCapabilities = lib.mkIf cfg.tun.enable [
                  "CAP_NET_ADMIN"
                  "CAP_NET_BIND_SERVICE"
                  "CAP_NET_RAW"
                ];
                CapabilityBoundingSet = lib.mkIf cfg.tun.enable [
                  "CAP_NET_ADMIN"
                  "CAP_NET_BIND_SERVICE"
                  "CAP_NET_RAW"
                ];
              };
            };
          })
          (lib.mkIf desktopCfg.enable {
            environment.systemPackages = [ desktopCfg.package ];
          })
        ];
      };
  };
}
