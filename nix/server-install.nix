{ lib, bash, coreutils, gnugrep, gnused, gawk, curl, systemd, shadow }:
''
  # These integration scripts track the Nix module, including when the binary
  # comes from an older CI release. Never ship the /opt installer into this package.
  cp ${../deploy/launch.sh} $out/launch
  cp ${../deploy/mihomo-server-user} $out/mihomo-server-user
  chmod +x $out/launch $out/mihomo-server-user
  cp $out/bin/mihomo-server $out/bin/mihomo-tun-exec
  mv $out/bin/mihomo-server $out/bin/.mihomo-server-unwrapped
  makeWrapper $out/bin/.mihomo-server-unwrapped $out/bin/mihomo-server \
    --set MIHOMO_SERVER_HELPER $out/mihomo-server-user \
    --prefix PATH : ${lib.makeBinPath [ bash coreutils gnugrep gnused gawk curl systemd shadow ]} \
    --run "$(cat ${./cli-guard.sh})"
  ln -s ../mihomo-server-user $out/bin/mihomo-server-user
  wrapProgram $out/mihomo-server-user \
    --prefix PATH : ${lib.makeBinPath [ bash coreutils gnugrep gnused gawk curl systemd shadow ]}
  wrapProgram $out/launch \
    --prefix PATH : ${lib.makeBinPath [ bash coreutils gnugrep gnused gawk systemd shadow ]}
''
