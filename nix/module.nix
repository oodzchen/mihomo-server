{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.mihomo-server;
  desktop = config.programs.mihomo-server-desktop;
in {
  imports = [
    (lib.mkRemovedOptionModule [ "services" "mihomo-server" "user" ]
      "Use services.mihomo-server.users; instances now run as systemd user services.")
    (lib.mkRemovedOptionModule [ "services" "mihomo-server" "dataDir" ]
      "Each user keeps XDG data. Set MIHOMO_SERVER_DATA_DIR in that user's env file for an override.")
    (lib.mkRemovedOptionModule [ "services" "mihomo-server" "listen" ]
      "Set MIHOMO_SERVER_LISTEN in each user's env file; default ports are assigned by slot.")
  ];
  options = {
    services.mihomo-server = {
      enable = lib.mkEnableOption "Mihomo Server with independent user instances";
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        description = "Complete service package; defaults to the CI precompiled release.";
      };
      users = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = "Existing non-root users whose instances start declaratively.";
      };
      linger = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Keep the selected users' services running without a login session.";
      };
      tun.enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Authorize selected users to use the shared system-wide TUN.";
      };
    };
    programs.mihomo-server-desktop = {
      enable = lib.mkEnableOption "Mihomo Server desktop client";
      package = lib.mkOption {
        type = lib.types.package;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.desktop;
        description = "Desktop client package.";
      };
    };
  };
  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      assertions = [
        { assertion = cfg.users != [] && !(builtins.elem "root" cfg.users);
          message = "services.mihomo-server.users must name at least one existing non-root user."; }
        { assertion = lib.all (user: builtins.hasAttr user config.users.users && config.users.users.${user}.isNormalUser) cfg.users;
          message = "services.mihomo-server.users must reference existing normal users."; }
      ];
      environment.systemPackages = [ cfg.package ];
      users.groups.mihomo-tun = {};
      users.users = lib.genAttrs cfg.users (_: {
        linger = cfg.linger;
        extraGroups = lib.mkIf cfg.tun.enable [ "mihomo-tun" ];
      });
      systemd.tmpfiles.rules = [
        "d /var/lib/mihomo-server 0755 root root -"
        "d /var/lib/mihomo-server/slots 1777 root root -"
      ] ++ lib.optionals cfg.tun.enable [
        "f /var/lib/mihomo-server/tun.lock 0640 root mihomo-tun -"
      ];
      security.wrappers.mihomo-tun-exec = lib.mkIf cfg.tun.enable {
        source = "${cfg.package}/bin/mihomo-tun-exec";
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
            if (link && !/^ms[0-9]+$/.test(link)) return polkit.Result.NOT_HANDLED;
            return polkit.Result.YES;
          }
        });
      '';
      systemd.user.services.mihomo-server = {
        description = "Mihomo Server user instance (managed by NixOS)";
        wantedBy = [ "default.target" ];
        unitConfig.ConditionUser = map (user: "|${user}") cfg.users;
        serviceConfig = {
          Type = "simple";
          Environment = [ "MIHOMO_SERVER_ARGS=" ];
          EnvironmentFile = "-%E/mihomo-server/env";
          ExecStart = "${cfg.package}/launch --multi-user $MIHOMO_SERVER_ARGS";
          Restart = "on-failure";
          RestartSec = 3;
          KillSignal = "SIGTERM";
          KillMode = "mixed";
          TimeoutStopSec = 30;
          UMask = "0077";
        };
      };
      # NixOS switches reload system services; explicitly refresh live user
      # managers too so their ExecStart follows the new immutable package.
      systemd.services.mihomo-server-user-refresh = {
        description = "Apply the NixOS Mihomo Server package to selected user managers";
        wantedBy = [ "multi-user.target" ];
        after = [ "systemd-tmpfiles-setup.service" "linger-users.service" ];
        restartTriggers = [ cfg.package config.systemd.user.units."mihomo-server.service".unit ];
        path = [ pkgs.systemd pkgs.coreutils ];
        serviceConfig = { Type = "oneshot"; RemainAfterExit = true; };
        # Also runs before this refresh unit is removed/changed: selected users
        # from the old generation must not keep an obsolete service alive.
        preStop = lib.concatMapStringsSep "\n" (user: ''
          if uid=$(id -u ${lib.escapeShellArg user} 2>/dev/null); then
            if [ -S "/run/user/$uid/bus" ]; then
              systemctl --user --machine=${lib.escapeShellArg "${user}@.host"} stop mihomo-server.service
            fi
          fi
        '') cfg.users;
        script = lib.concatMapStringsSep "\n" (user: ''
          uid=$(id -u ${lib.escapeShellArg user})
          if [ -S "/run/user/$uid/bus" ]; then
            systemctl --user --machine=${lib.escapeShellArg "${user}@.host"} daemon-reload
            systemctl --user --machine=${lib.escapeShellArg "${user}@.host"} restart mihomo-server.service
          fi
        '') cfg.users;
      };
    })
    (lib.mkIf desktop.enable { environment.systemPackages = [ desktop.package ]; })
  ];
}
