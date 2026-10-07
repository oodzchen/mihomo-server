{ lib, rustPlatform, buildNpmPackage, importNpmLock, fetchurl, gzip,
  makeWrapper, python3, bash, coreutils, gnugrep, gnused, gawk, curl, systemd,
  getent, version }:
let
  pin = builtins.fromJSON (builtins.readFile ../deploy/core-pin.json);
  coreArchive = fetchurl {
    url = lib.replaceStrings [ "{version}" ] [ pin.version ] pin.download;
    # Pin the uncompressed output, as in deploy/core-pin.json.
    postFetch = ''
      mv $out $out.gz
      ${gzip}/bin/gunzip -c $out.gz > $out
      rm $out.gz
    '';
    sha256 = pin.sha256;
  };
  web = buildNpmPackage {
    pname = "mihomo-server-web";
    inherit version;
    src = ../web;
    npmDeps = importNpmLock { npmRoot = ../web; };
    npmConfigHook = importNpmLock.npmConfigHook;
    installPhase = ''cp -r dist $out'';
  };
in rustPlatform.buildRustPackage {
  pname = "mihomo-server";
  inherit version;
  src = lib.cleanSource ../.;
  MIHOMO_SERVER_VERSION = version;
  cargoLock.lockFile = ../Cargo.lock;
  buildAndTestSubdir = "service";
  cargoTestFlags = [ "--lib" ];
  nativeBuildInputs = [ makeWrapper python3 ];
  postInstall = ''
    mkdir -p $out/resources/core $out/share/man/man1 \
      $out/share/bash-completion/completions $out/share/zsh/site-functions
    cp ${coreArchive} $out/resources/core/verge-mihomo
    chmod +x $out/resources/core/verge-mihomo
    cp -r ${web} $out/resources/web
    cp ${../examples/minimal.yaml} $out/resources/minimal.yaml
    cp ${../LICENSE} $out/LICENSE
    cp ${../LICENSES.txt} $out/LICENSES.txt
    cp ${../deploy/mihomo-server.1} $out/share/man/man1/
    cp ${../deploy/completions/mihomo-server.bash} $out/share/bash-completion/completions/mihomo-server
    cp ${../deploy/completions/_mihomo-server} $out/share/zsh/site-functions/
    cat > $out/resources/manifest.json <<EOF
    {"schema_version":1,"target":"x86_64-unknown-linux-gnu","core":{"version":"${pin.version}","sha256":"${pin.sha256}"},"licenses":{"primary":"LICENSE","inventory":"LICENSES.txt"}}
    EOF
  '' + import ./server-install.nix {
    inherit lib bash coreutils gnugrep gnused gawk curl getent systemd;
  };
  postFixup = builtins.readFile ./server-fixup.sh;
  meta = {
    description = "Mihomo supervisor and Web UI built from source";
    license = lib.licenses.gpl3Only;
    platforms = [ "x86_64-linux" ];
    mainProgram = "mihomo-server";
  };
}
