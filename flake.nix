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

        serverRelease = {
          version = "0.2.13";
          hash = "sha256-0SSrjiwL8W1ri3CkEtFvPPQesnO67pvKvveB79xHdCE=";
        };

        server-bin = pkgs.callPackage ./nix/server-bin.nix { release = serverRelease; };
        server-source = pkgs.callPackage ./nix/server-source.nix {
          version = "${serverRelease.version}-dev.${self.shortRev or "dirty"}";
        };

        desktopIntegration = ''
            echo '{"kind":"nix"}' > $out/nix-installation.json
            cat > $out/nix-install-service <<'EOF'
            #!${pkgs.runtimeShell}
            echo 'Enable services.mihomo-server and select services.mihomo-server.users in NixOS, then run nixos-rebuild switch.' >&2
            exit 1
            EOF
            chmod +x $out/nix-install-service
            cat > $out/nix-service-helper <<'EOF'
            #!${pkgs.runtimeShell}
            if [ "''${1:-}" = enable ]; then shift; set -- start "$@"; fi
            exec ${if system == "x86_64-linux" then "${server-bin}/mihomo-server-user" else "mihomo-server-user"} "$@"
            EOF
            chmod +x $out/nix-service-helper
            wrapProgram $out/bin/mihomo-server-desktop \
              --set MIHOMO_SERVER_HELPER $out/nix-service-helper \
              --set MIHOMO_SERVER_INSTALLER $out/nix-install-service
          '';

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
            makeWrapper
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

          postFixup = desktopIntegration;

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
            makeWrapper
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

          postFixup = desktopIntegration;

          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r bin share $out/
            runHook postInstall
          '';
        };
      in
      {
        packages = pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          default = server-bin;
          server-bin = server-bin;
          server-source = server-source;
        } // {
          desktop = if system == "x86_64-linux" then desktop-bin else desktop-source;
          desktop-bin = desktop-bin;
          desktop-source = desktop-source;
        };

        checks = pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          module = import ./nix/tests.nix { inherit self nixpkgs pkgs; };
          vm = import ./nix/vm-test.nix { inherit self pkgs; };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rustup
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
      nixosModules.default = import ./nix/module.nix { inherit self; };
    };
}
