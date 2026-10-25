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

        # Release hashes per Nix system; written by scripts/update_flake.py.
        serverRelease = {
          version = "0.3.11";
          hashes = {
            aarch64-linux = "sha256-KnMqJ5YdrapBc8yJcHDVB1UdslaRMTPHwuEk5zbDC0Y=";
            x86_64-linux = "sha256-rAKJcpuqOoZ0kKbHohAuL4IhN+5i6eU8dNzPqSm48Ug=";
          };
        };
        targets = {
          x86_64-linux = "x86_64-unknown-linux-gnu";
          aarch64-linux = "aarch64-unknown-linux-gnu";
        };
        target = targets.${system};

        server-bin = pkgs.callPackage ./nix/server-bin.nix {
          inherit target;
          inherit (serverRelease) version;
          hash = serverRelease.hashes.${system};
        };
        server-source = pkgs.callPackage ./nix/server-source.nix {
          inherit target;
          version = "${serverRelease.version}-dev.${self.shortRev or "dirty"}";
        };
        released = serverRelease.hashes ? ${system};
      in
      {
        # A system without a published release yet builds from source.
        packages = pkgs.lib.optionalAttrs (targets ? ${system}) ({
          default = if released then server-bin else server-source;
          server-source = server-source;
        } // pkgs.lib.optionalAttrs released { server-bin = server-bin; });

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
