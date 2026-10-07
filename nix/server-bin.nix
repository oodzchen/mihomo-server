{ lib, stdenv, fetchurl, autoPatchelfHook, makeWrapper, python3, bash, coreutils,
  gnugrep, gnused, gawk, curl, getent, systemd, release }:
stdenv.mkDerivation {
  pname = "mihomo-server";
  inherit (release) version;
  src = fetchurl {
    url = "https://github.com/oodzchen/mihomo-server/releases/download/v${release.version}/mihomo-server-v${release.version}-x86_64-unknown-linux-gnu.tar.gz";
    inherit (release) hash;
  };
  nativeBuildInputs = [ autoPatchelfHook makeWrapper python3 ];
  buildInputs = [ stdenv.cc.cc.lib ];
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    # Verify the published bundle before patching any of its files.
    sha256sum -c checksums.sha256
    mkdir -p $out
    cp -r bin resources share LICENSE LICENSES.txt $out/
    runHook postInstall
  '';
  postInstall = import ./server-install.nix {
    inherit lib bash coreutils gnugrep gnused gawk curl getent systemd;
  };
  postFixup = builtins.readFile ./server-fixup.sh;
  meta = {
    description = "Mihomo supervisor with Web UI and a bundled core (CI release)";
    license = lib.licenses.gpl3Only;
    platforms = [ "x86_64-linux" ];
    mainProgram = "mihomo-server";
  };
}
