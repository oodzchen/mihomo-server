{ self, nixpkgs, pkgs }:
let
  system = nixpkgs.lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      self.nixosModules.default
      ({ ... }: {
        system.stateVersion = "26.05";
        boot.loader.grub.enable = false;
        fileSystems."/" = { device = "/dev/vda"; fsType = "ext4"; };
        users.users.alice.isNormalUser = true;
        users.users.bob.isNormalUser = true;
        services.mihomo-server = { enable = true; users = [ "alice" "bob" ]; };
      })
    ];
  };
  cfg = system.config;
  withoutTun = (system.extendModules {
    modules = [{ services.mihomo-server = { linger = false; tun.enable = false; }; }];
  }).config;
in
assert pkgs.lib.all (entry: entry.assertion) cfg.assertions;
assert cfg.users.users.alice.linger;
assert builtins.elem "mihomo-tun" cfg.users.users.alice.extraGroups;
assert cfg.security.wrappers.mihomo-tun-exec.permissions == "0750";
assert !withoutTun.users.users.alice.linger;
assert !(builtins.elem "mihomo-tun" withoutTun.users.users.alice.extraGroups);
assert !(withoutTun.security.wrappers ? mihomo-tun-exec);
pkgs.runCommand "mihomo-server-nixos-module-check" {} ''
  unit=${cfg.systemd.user.units."mihomo-server.service".unit}/mihomo-server.service
  grep -F 'ConditionUser=|alice' "$unit"
  grep -F 'ConditionUser=|bob' "$unit"
  grep -F '${cfg.services.mihomo-server.package}/launch --multi-user' "$unit"
  test -x ${cfg.services.mihomo-server.package}/bin/mihomo-server
  mkdir $out
  cp "$unit" $out/mihomo-server.service
''
