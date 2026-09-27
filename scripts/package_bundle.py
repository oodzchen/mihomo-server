#!/usr/bin/env python3
"""Prepare a pinned local Linux bundle; never download or alter runtime data."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-unknown-linux-gnu"
GEO_NAMES = {"Country.mmdb", "ASN.mmdb", "geoip.dat", "geosite.dat", "geoip.metadb", "GeoSite.dat"}
MAX_GEO_FILE = 128 * 1024 * 1024


def geo_seeds(args):
    directory, manifest = getattr(args, "geo_dir", None), getattr(args, "geo_manifest", None)
    if not directory and not manifest:
        return None, {}
    if not directory or not manifest:
        raise ValueError("Geo directory and integrity manifest must be supplied together")
    directory, manifest = Path(directory).absolute(), Path(manifest).absolute()
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError("Geo source directory must be real")
    if manifest.is_symlink() or not manifest.is_file() or manifest.stat().st_size > 65536:
        raise ValueError("Geo manifest must be a regular file up to 64 KiB")
    pins = json.loads(manifest.read_text())
    if not isinstance(pins, dict) or len(pins) > len(GEO_NAMES):
        raise ValueError("Geo pins must be a map of supported filenames")
    for name, pin in pins.items():
        if name not in GEO_NAMES or not isinstance(pin, dict) or set(pin) != {"bytes", "sha256"}:
            raise ValueError("Unsupported Geo filename or pin fields")
        if type(pin["bytes"]) is not int or not 0 < pin["bytes"] <= MAX_GEO_FILE:
            raise ValueError("Geo file must be 1 byte to 128 MiB")
        if not isinstance(pin["sha256"], str) or not re.fullmatch(r"[0-9a-fA-F]{64}", pin["sha256"]):
            raise ValueError("Invalid Geo SHA-256 pin")
        source = directory / name
        if source.is_symlink() or not source.is_file() or source.stat().st_size != pin["bytes"]:
            raise ValueError("Geo source size/type differs from pin")
        if sha256(source) != pin["sha256"].lower():
            raise ValueError("Geo source SHA-256 differs from pin")
        pin["sha256"] = pin["sha256"].lower()
    if sum(pin["bytes"] for pin in pins.values()) > 256 * 1024 * 1024:
        raise ValueError("Geo seeds exceed 256 MiB total")
    return directory, pins


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def regular_tree(directory):
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError(f"Expected real directory: {directory}")
    for path in directory.rglob("*"):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise ValueError(f"Unsafe resource entry: {path}")


def executable(path):
    if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(f"Expected regular executable: {path}")
    with path.open("rb") as stream:
        header = stream.read(20)
    if (len(header) != 20 or header[:6] != b"\x7fELF\x02\x01"
            or int.from_bytes(header[18:20], "little") != 62):
        raise ValueError(f"Expected Linux x86_64 ELF executable: {path}")


def package(args):
    if args.target != TARGET:
        raise ValueError(f"Initial bundle supports only {TARGET}")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", args.core_sha256):
        raise ValueError("Provide the expected 64-digit core SHA-256")
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,100}", args.core_version):
        raise ValueError("Provide an explicit pinned core version")
    geo_directory, geo = geo_seeds(args)
    output = Path(args.output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("Bundle output already exists; use a fresh version directory")
    core = Path(args.mihomo).absolute()
    executable(core)
    if sha256(core) != args.core_sha256.lower():
        raise ValueError("Core SHA-256 does not match the supplied pin")
    version = subprocess.run([str(core), "-v"], check=True, capture_output=True,
                             text=True, timeout=10).stdout
    if not re.search(r"\bMihomo(?: Meta)? " + re.escape(args.core_version) + r"(?:\s|$)", version):
        raise ValueError("Core version does not match the supplied pin")
    if args.build:
        subprocess.run(["npm", "--prefix", "web", "ci"], cwd=ROOT, check=True)
        subprocess.run(["npm", "--prefix", "web", "run", "build"], cwd=ROOT, check=True)
        subprocess.run(["cargo", "build", "-p", "mihomo-server", "--release", "--locked",
                        "--target", args.target], cwd=ROOT, check=True)
    service = Path(args.service or ROOT / "target" / args.target / "release" / "mihomo-server").absolute()
    web = Path(args.web_dir or ROOT / "web" / "dist").absolute()
    executable(service)
    regular_tree(web)
    if not (web / "index.html").is_file():
        raise ValueError("Web directory needs a built index.html")
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".mihomo-bundle-", dir=output.parent))
    try:
        (stage / "bin").mkdir()
        (stage / "resources" / "core").mkdir(parents=True)
        shutil.copyfile(service, stage / "bin" / "mihomo-server")
        shutil.copyfile(core, stage / "resources" / "core" / "verge-mihomo")
        for binary in [stage / "bin" / "mihomo-server", stage / "resources" / "core" / "verge-mihomo"]:
            binary.chmod(0o755)
        # Verify the published copy as well as the input before copying.
        if sha256(stage / "resources" / "core" / "verge-mihomo") != args.core_sha256.lower():
            raise ValueError("Core changed while preparing bundle")
        shutil.copytree(web, stage / "resources" / "web")
        shutil.copyfile(ROOT / "examples" / "minimal.yaml", stage / "resources" / "minimal.yaml")
        manifest = {"schema_version": 1, "target": args.target,
                    "core": {"version": args.core_version, "sha256": args.core_sha256.lower()}}
        if geo:
            (stage / "resources" / "geo").mkdir()
            for name, pin in geo.items():
                destination = stage / "resources" / "geo" / name
                shutil.copyfile(geo_directory / name, destination)
                destination.chmod(0o644)
                if destination.stat().st_size != pin["bytes"] or sha256(destination) != pin["sha256"]:
                    raise ValueError("Geo source changed while preparing bundle")
            manifest["geo"] = geo
        (stage / "resources" / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        shutil.copyfile(ROOT / "deploy" / "launch.sh", stage / "launch")
        (stage / "launch").chmod(0o755)
        shutil.copyfile(ROOT / "deploy" / "mihomo-server.service", stage / "mihomo-server.service")
        shutil.copyfile(ROOT / "LICENSE", stage / "LICENSE")
        (stage / "docs").mkdir()
        for name in ["DEPLOYMENT.md", "UPSTREAM.md"]:
            shutil.copyfile(ROOT / "docs" / name, stage / "docs" / name)
        lines = [f"{sha256(path)}  {path.relative_to(stage).as_posix()}\n"
                 for path in sorted(stage.rglob("*")) if path.is_file()]
        (stage / "checksums.sha256").write_text("".join(lines))
        stage.rename(output)
    finally:
        if stage.exists():
            shutil.rmtree(stage)
    print(f"Prepared {output}; target={args.target}; core={args.core_version}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--mihomo", required=True, help="Existing independent core executable")
    parser.add_argument("--core-version", required=True, help="Pinned version, e.g. v1.19.31")
    parser.add_argument("--core-sha256", required=True, help="Expected SHA-256 of the uncompressed core")
    parser.add_argument("--output", required=True, help="Fresh bundle directory; existing output is rejected")
    parser.add_argument("--build", action="store_true", help="Build locked frontend and release service first")
    parser.add_argument("--service", help="Use a prebuilt service instead of the release build location")
    parser.add_argument("--web-dir", help="Use prebuilt assets instead of web/dist")
    parser.add_argument("--geo-dir", help="Optional directory of existing Geo seed files")
    parser.add_argument("--geo-manifest", help="Expected Geo filename -> {bytes, sha256} pins; requires --geo-dir")
    args = parser.parse_args()
    try:
        package(args)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Bundle preparation failed: {error}\n")


if __name__ == "__main__":
    main()
