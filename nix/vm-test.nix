{ self, pkgs }:
# Boots the module: declared user instances, TUN capability, per-user DNS
# polkit rule, Nix-owned updates, and a switch that adds a user whose manager
# predates their mihomo-tun membership (the launcher's setuid sg path).
pkgs.testers.runNixOSTest {
  name = "mihomo-server-module";
  nodes.machine = { lib, pkgs, ... }: {
    imports = [ self.nixosModules.default ];
    virtualisation.memorySize = 2048;
    virtualisation.cores = 2;
    services.resolved.enable = true;
    environment.systemPackages = [ pkgs.libcap ];
    users.users = {
      alice = { isNormalUser = true; uid = 1000; };
      bob = { isNormalUser = true; uid = 1001; };
      # Lingering before the module selects her: her manager starts without the group.
      carol = { isNormalUser = true; uid = 1002; linger = true; };
    };
    services.mihomo-server = { enable = true; users = [ "alice" "bob" ]; };
    specialisation.carol.configuration = {
      services.mihomo-server.users = lib.mkForce [ "alice" "bob" "carol" ];
    };
  };
  testScript = ''
    import re
    import shlex

    uids = {"alice": 1000, "bob": 1001, "carol": 1002}

    def run(user, command):
        line = f"XDG_RUNTIME_DIR=/run/user/{uids[user]} {command} 2>&1"
        return machine.execute(f"su -l {user} -c {shlex.quote(line)}")

    def ready(user):
        machine.wait_for_unit("mihomo-server.service", user)
        # Released v0.2.13 can probe listeners before they bind; one restart
        # recovers it (fixed in the current source).
        for attempt in range(2):
            try:
                machine.wait_until_succeeds(
                    f"su -l {user} -c 'XDG_RUNTIME_DIR=/run/user/{uids[user]} mihomo-server status'", timeout=60)
                break
            except Exception:
                if attempt:
                    raise
                print(run(user, "mihomo-server-user logs -n 50 --no-pager")[1])
                run(user, "systemctl --user restart mihomo-server.service")
        status, info = run(user, "mihomo-server-user info")
        assert status == 0, info
        assert re.search(r"^tun: *available ", info, re.M), info
        unit = machine.succeed(f"systemctl --user -M {user}@ show mihomo-server.service -p FragmentPath -p ExecStart")
        assert "FragmentPath=/etc/systemd/user/mihomo-server.service" in unit, unit
        assert "/nix/store/" in unit and "/launch --multi-user" in unit, unit

    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("mihomo-server-user-refresh.service")

    with subtest("declared users run TUN-capable instances from the store"):
        for user in ["alice", "bob"]:
            ready(user)
        assert "cap_net_admin" in machine.succeed("getcap /run/wrappers/bin/mihomo-tun-exec")
        status, _ = machine.execute("systemctl --user -M carol@ is-active mihomo-server.service")
        assert status != 0, "carol is not declared yet"

    with subtest("program updates and uninstall belong to Nix"):
        for command in ["mihomo-server update", "mihomo-server uninstall --yes"]:
            status, output = run("alice", command)
            assert status != 0 and "nixos-rebuild switch" in output, output

    with subtest("members may set DNS only on their own ms<uid> link"):
        machine.succeed("ip link add ms1000 type dummy && ip link set ms1000 up")
        status, output = run("alice", "resolvectl dns ms1000 192.0.2.53")
        assert status == 0, output
        status, output = run("bob", "resolvectl dns ms1000 192.0.2.54")
        assert status != 0, output
        assert "192.0.2.54" not in machine.succeed("resolvectl dns ms1000")
        machine.succeed("ip link del ms1000")

    with subtest("a switch adds a user whose manager predates the group"):
        machine.wait_for_unit("user@1002.service")
        manager = machine.succeed("systemctl show user@1002.service -p MainPID --value").strip()
        gid = machine.succeed("getent group mihomo-tun | cut -d: -f3").strip()
        # An old script installation's user unit shadows the module's unit.
        machine.succeed("install -D -o alice -m 644 /etc/systemd/user/mihomo-server.service "
                        "/home/alice/.config/systemd/user/mihomo-server.service")
        machine.succeed("/run/booted-system/specialisation/carol/bin/switch-to-configuration test")
        groups = machine.succeed(f"grep ^Groups: /proc/{manager}/status")
        assert gid not in groups.split(), "carol's manager must still lack the group: " + groups
        refresh = machine.succeed("journalctl -b -u mihomo-server-user-refresh.service --no-pager")
        assert "shadowed by /home/alice/.config/systemd/user/mihomo-server.service" in refresh, refresh
        machine.succeed("rm /home/alice/.config/systemd/user/mihomo-server.service")
        machine.succeed("systemctl --user -M alice@ daemon-reload")
        for user in ["alice", "bob", "carol"]:
            ready(user)
  '';
}
