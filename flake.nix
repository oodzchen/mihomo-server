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

        serverRelease = {
          version = "0.3.2";
          hash = "sha256-m1IBAGzpj21NSWyrelPgb9SRfKsg+0zsfvWow9IKyiU=";
        };

        server-bin = pkgs.callPackage ./nix/server-bin.nix { release = serverRelease; };
        server-source = pkgs.callPackage ./nix/server-source.nix {
          version = "${serverRelease.version}-dev.${self.shortRev or "dirty"}";
        };
      in
      {
        packages = pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          default = server-bin;
          server-bin = server-bin;
          server-source = server-source;
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
          ];
        };
      }
    ) // {
      nixosModules.default = import ./nix/module.nix { inherit self; };
    };
}
